# 架构

```
浏览器（Vue 3 + Univer + Element Plus）
   │  HTTP / JSON；SSE（协作与 AI 流式回答）
   ▼
gridlake（单进程：Rust + axum）
   ├─ 认证与权限中间件（auth.rs）：登录 → 路由权限 → 数据范围 → 审计
   ├─ 业务模块：数据集 / 透视 / 公式下推 / SQL / 工作簿 / 协作 / AI / 审批 / 连接 / 推送 …
   └─ DuckDB（内置，单个数据文件）
        data/ledger.duckdb      全部业务数据：数据集、工作簿、版本、账号、审计…
        data/tmp/spill/         内存不够时排序 / 聚合的落盘目录（可随时清空）
        data/tmp/cursors.duckdb 大数据表查询游标的位置表（进程重启即作废，不进备份）
        data/backups/           自动备份
        data/secret.key         加密数据连接密码的密钥（备份时一并保管）
```

前端构建产物也可以由同一个进程直接提供（`LEDGER_WEB_DIR`），所以最小部署只有一个进程加一个数据目录。

## 核心思路

1. **界面是 Excel，数据在服务端。** 普通工作表、公式、图表在浏览器里算；引用大数据集的透视、公式、数据表、图表**下推到 DuckDB**，浏览器只取屏幕上那一屏。
2. **大数据表用「游标」。** 打开数据表（带筛选 / 排序 / 搜索）时服务端先算出一个游标（行位置表，放在临时库里），之后按位置取行，滚动不重复排序；相同条件的游标会复用。
3. **重计算要排队。** 透视、SQL、异常发现、带筛选或排序的游标会吃满 CPU 和内存，所以有并发闸门（`LEDGER_HEAVY_QUERIES`）：超出的排队，排队太久返回 503「服务繁忙」，客户端断开会自动退队。
4. **数据范围（行级权限）在统一出口生效。** 规则按（用户，维度）设置可见取值，数据集绑定（过滤列，维度）；受限用户的请求在「加载数据集定义」时把数据来源换成带过滤条件的子查询（`scope.rs`），所以透视、公式、SQL、AI、导出、游标全部自动受限；会绕过过滤的写操作对受限用户默认关闭（`auth.rs` 的 `RESTRICTED_OK` 白名单）。
5. **SQL 只读且受控。** 语句先交给 DuckDB 自带的解析器转成语法树，按白名单检查（只能 SELECT、只能读数据集、禁止读文件 / 环境变量 / 配置的函数），在独立连接和事务里执行并设时限。

## 后端模块

见 [server/README.md](../server/README.md) 的模块表。几个关键的横切点：

| 位置 | 作用 |
|---|---|
| `main.rs` | 路由注册、中间件顺序（请求进入时：跨域 → 日志 → 压缩 → 指标 → 认证）、启动检查 |
| `auth.rs` | 四种登录方式、`ROUTES` 权限表、`RESTRICTED_OK` / `FULL_ONLY`、操作审计 |
| `db.rs` | 连接管理、并发闸门、启动时建表 / 升级 / 结构版本检查 |
| `schema.rs` | 建表与增量升级；`SCHEMA_VERSION`（被更新版本写过的数据文件，旧版拒绝打开） |
| `metrics.rs` | Prometheus 指标与慢请求日志 |
| `dataset*.rs` / `views.rs` | 数据集导入、游标、计算列、关联视图、回收站 |
| `pivot.rs` | 透视：`GROUPING SETS` 一次算出所有层级与总计 |
| `query.rs` / `sqlq/` | 公式下推（D 系列）、SQL 工作台 |
| `collab.rs` | 实时协作（SSE 推送 + POST 提交，按版本号收敛冲突） |
| `ai/` / `mcp/` | AI 助手（OpenAI 兼容接口，方案卡确认后才写入）、MCP 只读服务 |

## 列类型

数据集的每一列有一个类型（`kind`），决定存储类型、能不能求和、筛选怎么比较、界面怎么显示。定义集中在 `server/src/kinds.rs` 与 `web/src/features/excel/dataset/column-kinds.ts`，两边一一对应；判断「是不是数值列」一律用 `kinds::is_numeric` / `isNumericKind`，新增类型时只改这两个文件。

| kind | 含义 | 存储类型 |
|---|---|---|
| `text` | 文本 | VARCHAR |
| `id` | 编号（订单号、手机号：长数字串 / 前导零，按文本保存不丢位） | VARCHAR |
| `integer` | 整数（数量、件数：精确到个位） | BIGINT |
| `number` | 数值 | DOUBLE |
| `money` | 金额 | DOUBLE |
| `datetime` | 日期时间，统一写成 `2026-09-15 10:00:00` 形式的**文本**（透视按年 / 季 / 月 / 周分组、切片器、AI 与 MCP 的说明都按这个约定） | VARCHAR |
| `boolean` | 是 / 否 | BOOLEAN |

类型从哪里来：CSV / XLSX 按前 2000 行样本推断（全是整数 → `integer`，全是 `true` / `false` → `boolean`）；新建时再用全量数据核对一次，样本之后出现小数的整数列改成数值列、出现其他写法的是否列改成文本列，不让样本之外的值按空值入库。Parquet / JSON 用文件自带的类型（转 CSV 时记在暂存文件旁的 `.kinds.json` 里，预览时优先）；SQL 结果另存为数据集时按结果的 SQL 类型。超过 2^53 的整数在接口里以文本下发（浏览器的 JSON 数字会丢掉末几位）。

## 外部数据集（直接查询 Parquet）

`server/src/external.rs`：管理员把收件目录（`LEDGER_INBOX_ROOTS`）里的一组 Parquet 文件登记成只读数据集（登记表里 `kind = 'external'`），数据库里是一个视图：

```sql
SELECT <各列，日期换成文本>, file_index::INTEGER AS __fi, file_row_number AS __fr,
       (<每个文件的起始位置>)[file_index + 1] + file_row_number AS __rid
FROM read_parquet([<文件列表>], union_by_name = true, hive_partitioning = …)
```

文件列表和每个文件的行数在创建 / 刷新时确定并记在定义里，所以列数据集、给行数都不读文件。浏览走两条快路径（`dataset.rs` 的游标）：直通游标把行位置区间换算成 `(__fi, __fr)` 的范围，筛选 / 排序后的位置表存 `(fi, fr)` 而不是行号，取一屏时按它关联——两种条件都能下推到 Parquet 扫描，只读要显示的那一段；按计算出来的 `__rid` 取数要扫全表（1000 万行上约 150 毫秒对 5 ~ 15 毫秒），只留给关联视图等通用路径。

## 分享链接

带 `x-share-token` 的请求在 `auth.rs` 的 `guard` 里走独立分支（`share_request`），**不经过登录与权限表**：只能调用 `share.rs` 里 `ALLOWED` 登记的 10 个只读接口（工作簿内容 / 元信息、数据集列表、游标 / 取行 / 统计 / 取值 / 聚合 / 透视）。工作簿接口只认被分享的那一个；数据集接口只认「这个工作簿用到的数据集」——打开分享页（读内容）时用 `template::refs::collect` 从工作簿内容里算出引用的数据集（id 与名称）并记在 `ledger_share.datasets`。请求在**创建人的数据范围**内执行（`resolve_scope` 用创建人身份），所以分享不会比创建人多看一行。前端 `/share/:token` 复用表格引擎与图表 / 指标卡 / 切片器部件，用演示模式的编辑拦截（`present-guard`）挡住编辑。

## 场景包（可选功能）

核心是通用的数据集 / 透视表 / 公式 / SQL / 仪表盘 / 协作。项目最早的使用场景——电商资金台账（导入抖店资金流水、店铺档案、月结锁账与审批、余额勾稽、提现 / 银行对账、经营分析）——做成了**场景包**，和核心分开：

| | 位置 |
|---|---|
| 后端 | `server/src/ecommerce/`（业务逻辑）、`server/src/routes/ecommerce.rs`（处理函数）、`ecommerce::router()`（路由，没启用时不注册，访问得到 404） |
| 前端 | `web/src/packs/ecommerce/`（接口封装、`use-ledger.ts`、台账 / 月结 / 明细 / 分析的组件、汇总表模板） |
| 开关 | 环境变量 `LEDGER_PACKS`：不设置 = 库里已有台账数据才启用（老部署升级后功能不消失，新库不启用）；`ecommerce` 启用；`none` 全关。启动时确定一次，改了要重启 |
| 前端怎么知道 | `GET /api/ledger/me` 返回 `packs: { ecommerce: bool }`，界面据此显示 / 隐藏入口（数据中心菜单、命令面板、「台账」功能区），没启用时不会请求台账接口 |
| 随场景包出现 / 消失的东西 | 内置的「资金流水」数据集；电商专用的内置模板（退款与售后分析、千川投放 ROI、多平台利润表） |

边界规则：**核心不能依赖场景包的业务逻辑**。通用的东西放核心——读 CSV / Excel 与编码识别、金额日期解析、取值转 JSON 在 `server/src/tabular.rs`；金额格式化在 `web/src/utils/format-money.ts`；操作人在 `web/src/features/excel/workspace/use-operator.ts`。新增一个领域场景（比如进销存、广告投放）时，照着这个结构加一个包：自己的目录、自己的路由函数、自己的开关名（加进 `config::KNOWN_PACKS`）、界面入口按 `/me` 的 `packs` 显示。

## 前端结构

`web/src`：`api/`（接口封装）、`features/excel/`（与界面无关的逻辑：透视、数据表、公式、协作、xlsx 导入导出、AI 动作……，多数可直接单元测试）、`components/excel/`（面板与对话框）、`views/`（页面）、`stores/`（Pinia）。表格内核 Univer 通过 `lib/univer.ts` 装配。

## 数据流示例：打开一张 2 亿行的订单表并按店铺筛选

1. 前端 `POST /api/ledger/datasets/{id}/cursor`（筛选条件）→ 服务端（受限用户先套数据范围）算出游标，返回总行数与游标 id；
2. 滚动时 `GET …/rows?cursor=&offset=&limit=` 按位置取一屏；
3. 筛选后的表头统计、列值下拉分别走 `/stats`、`/distinct`；
4. 需要时 `POST …/pivot` 做透视，结果写进工作簿的一个页签（普通单元格）。

## 测试

后端单元测试（`cargo test`）+ 端到端测试（`tests/e2e/`，每个文件一个临时库的服务）+ 规模与浸泡测试（`tests/bench/`）；前端单元测试 + Chrome 界面测试。详见 [CONTRIBUTING.md](../CONTRIBUTING.md)。
