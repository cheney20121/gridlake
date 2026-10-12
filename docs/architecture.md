# 架构

```
浏览器（Vue 3 + Univer + Element Plus）
   │  HTTP / JSON；SSE（协作与 AI 流式回答）
   ▼
ledger-server（单进程：Rust + axum）
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
4. **数据范围（行级权限）在统一出口生效。** 受限用户的请求在「加载数据集定义」时把数据来源换成带过滤条件的子查询（`scope.rs`），所以透视、公式、SQL、AI、导出、游标全部自动受限；会绕过过滤的写操作对受限用户默认关闭（`auth.rs` 的 `RESTRICTED_OK` 白名单）。
5. **SQL 只读且受控。** 语句先交给 DuckDB 自带的解析器转成语法树，按白名单检查（只能 SELECT、只能读数据集、禁止读文件 / 环境变量 / 配置的函数），在独立连接和事务里执行并设时限。

## 后端模块

见 [ledger-server/README.md](../ledger-server/README.md) 的模块表。几个关键的横切点：

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

## 前端结构

`demo-web-vue3/src`：`api/`（接口封装）、`features/excel/`（与界面无关的逻辑：透视、数据表、公式、协作、xlsx 导入导出、AI 动作……，多数可直接单元测试）、`components/excel/`（面板与对话框）、`views/`（页面）、`stores/`（Pinia）。表格内核 Univer 通过 `lib/univer.ts` 装配。

## 数据流示例：打开一张 2 亿行的订单表并按店铺筛选

1. 前端 `POST /api/ledger/datasets/{id}/cursor`（筛选条件）→ 服务端（受限用户先套数据范围）算出游标，返回总行数与游标 id；
2. 滚动时 `GET …/rows?cursor=&offset=&limit=` 按位置取一屏；
3. 筛选后的表头统计、列值下拉分别走 `/stats`、`/distinct`；
4. 需要时 `POST …/pivot` 做透视，结果写进工作簿的一个页签（普通单元格）。

## 测试

后端单元测试（`cargo test`）+ 端到端测试（`tests/e2e/`，每个文件一个临时库的服务）+ 规模与浸泡测试（`tests/bench/`）；前端单元测试 + Chrome 界面测试。详见 [CONTRIBUTING.md](../CONTRIBUTING.md)。
