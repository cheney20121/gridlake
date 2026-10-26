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

已有数据集改类型（`server/src/dataset_alter.rs`，只限普通数据表）：`ALTER TABLE … ALTER COLUMN … SET DATA TYPE … USING (…)` 就地转换，行号不变，按行号引用数据的地方（取行、行详情）都还对得上。文本 → 其他类型用与导入**同一个**转换表达式（`dataset_import::convert_sql`，认千分位、`是 / 否` 等写法）；数值 → 整数只收整数值（`1.5` 算转不过来，不四舍五入）；数值 → 文本去掉 `.0`。先预览：数一数有多少个非空值会变成空值、举例，以及几行转换前后的样子；有值转不过来时必须带 `acceptLoss` 才执行。执行包在 `guarded_change` 里：改完后重新检查依赖这张表的计算列、关联视图，有失效的整体回滚。数值列改成非数值列时提示「透视求和、公式条件按数值比较的地方会变」。

## 外部数据集（直接查询 Parquet）

`server/src/external.rs`：管理员把收件目录（`LEDGER_INBOX_ROOTS`）里的一组 Parquet 文件登记成只读数据集（登记表里 `kind = 'external'`），数据库里是一个视图：

```sql
SELECT <各列，日期换成文本>, file_index::INTEGER AS __fi, file_row_number AS __fr,
       (<每个文件的起始位置>)[file_index + 1] + file_row_number AS __rid
FROM read_parquet([<文件列表>], union_by_name = true, hive_partitioning = …)
```

文件列表和每个文件的行数、大小、修改时间在创建 / 刷新时确定并记在定义里，所以列数据集、给行数都不读文件。设了自动检查的，后台任务 `external::schedule` 每 20 秒看一次谁到点了，到点后先只比对文件系统（`files_changed`：文件列表、大小、修改时间），有变化才走和手动刷新相同的 `rebuild`（重建视图，用 `guarded_change` 保护计算列和关联视图）；检查时间和失败原因也记在定义里。浏览走两条快路径（`dataset.rs` 的游标）：直通游标把行位置区间换算成 `(__fi, __fr)` 的范围，筛选 / 排序后的位置表存 `(fi, fr)` 而不是行号，取一屏时按它关联——两种条件都能下推到 Parquet 扫描，只读要显示的那一段；按计算出来的 `__rid` 取数要扫全表（1000 万行上约 150 毫秒对 5 ~ 15 毫秒），只留给关联视图等通用路径。

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

## 事件 Webhook

`server/src/webhook.rs`。业务代码在事件发生的地方调用 `webhook::emit`（异步处理函数里）或 `webhook::emit_with`（已有数据库连接的阻塞代码里，如连接器同步收尾、SQL 重算）：给每个订阅了这个事件、启用中的 Webhook 往 `ledger_webhook_delivery` 插一条 `pending` 记录（请求体在这时定下来，重发时不变），再唤醒后台发送任务 `webhook::run`。「哪些事件有人订阅」缓存在 `Db.webhooks`（增删改时清空），没人订阅时 `emit` 直接返回、不碰数据库，所以埋点几乎没有开销。

发送任务每次取 16 条到点的记录并发发送，按结果改成 `success`、`pending`（排下次重试，`next_at` 按 1 分钟 → 6 小时退避）或 `failed`（6 次用完）；没有新事件时每 15 秒看一次到点的重试。签名是 `HMAC-SHA256(密钥, "<秒级时间戳>.<请求体>")`，时间戳放在签名头里，接收方据此拒绝重放。HTTP 客户端用 `netguard::client_builder()`（连接时过滤地址、重定向逐跳检查）。新增事件：在 `EVENTS` 里登记名称和说明（名称是对外约定，只增不改），在发生的地方调用 `emit`，并在部署手册的事件表里补一行。

## 数据概况

`server/src/profile.rs`。概况把所有列要算的统计拼进**一条** `SELECT`（每列十来个聚合：`count`、`approx_count_distinct`、`min` / `max`、数值列的 `sum` / `avg` / `stddev_samp` / `approx_quantile`、`count(*) FILTER (WHERE …)` 数零、负数、空白、认不出的日期），DuckDB 一次扫描、多核并行算完；第二条 `SELECT` 只对估计不同值不超过 10 万的列做精确的 `count(DISTINCT …)`（HyperLogLog 在取值很少时误差反而大，而这些列的哈希表很小）。数据来源用 `dataset::dataset()`（数据范围受限时已换成带过滤的子查询），条件用 `dataset::rows_sql()`（与游标同一套筛选、关键字和汇总项规则），所以概况看到的行和数据表一致。单列分布是两三条按分档 / 日期前缀 / 取值的 `GROUP BY`。两个接口都走重计算并发闸门（`blocking_heavy`）。

## AI 助手的数据工具

`ai/tools.rs`。AI 工具分两类：查询类直接执行、结果回给模型；产出类只生成「建议」（kind + 参数 + 预览），用户在卡片上点「应用」后由前端调用与手动操作相同的接口（`features/excel/ai/workbook.ts`），权限检查也就和手动操作一致。数据概况 `profile_dataset` 是查询类，复用 `profile::overview`，同时开放给 MCP。清洗 `propose_clean` 和窗口计算 `create_window_dataset` 是产出类：服务端先用 `clean::preview` / `window::preview` 算出受影响行数、改前改后或新列的前几行放进卡片，模型给错参数时把错误回给它修正；因为会修改 / 新建服务端数据，不能放进方案卡（方案卡要能整体撤回），也不对 MCP 开放（MCP 没有人工确认）。数据范围受限的用户不能提这两种建议（`scope::restricted_now`），与对应接口对受限用户关闭一致。

数据集页面的数据表把正在看的数据集和当前筛选 / 搜索 / 排序发布到 `features/excel/dataset/active-view.ts`，数据集页面在前台时 `collectContext` 用它代替工作表和选区作为界面上下文（`ai/mod.rs` 的 `DatasetViewCtx`），助手据此理解「这些 / 当前」指的是什么。

清洗只能撤销最近一次，所以撤销卡片前先读 `GET …/clean`，确认最近一次的说明和时间与应用时记下的一致，否则拒绝，避免撤销到后来的另一次清洗。应用 / 撤销后调用 `markDataChanged`（`features/excel/dataset/data-changes.ts`），打开着的数据表据此重新建游标——查找替换不改行数，不能靠数据集列表里的行数判断。

## 分类汇总

数据不重排、不复制：游标按「分组列, 原来的排序」排好，每组的行就连在一起；`dataset::groups` 用同一个分组表达式、同样的 `ASC / DESC NULLS LAST` 一条 `GROUP BY` 返回每组的行数和汇总（汇总表达式与汇总行共用 `total_expr`），前端按各组行数在「显示行」（数据行 + 每组一行小计）和数据行之间换算（`big-grid/group-map.ts`，前缀和 + 二分查找），取数、选区统计、复制都换算回数据行号。各组行数之和和游标总行数对不上时（换游标的瞬间）先不显示小计。超过 1 万组拒绝。折叠的组只占一行小计：换算表按「展开的组的行数 + 1」做前缀和，取数只请求屏幕上真正显示的数据行（`dataSpans`，可能是几段不连续的区间，`RowCache.ensureSpans`），选中折叠组的小计行代表整组。导出 Excel 时服务端从游标重算各组（与游标同序），逐行写数据、每组后面写 `SUBTOTAL` 小计；rust_xlsxwriter 的常量内存模式不写行的 outlineLevel / collapsed，所以保存后由 `xlsx_outline.rs` 流式改写数据页签的 `<row>` 标签补上（其他文件原样复制）。

多级（最多 3 级）时游标按「第 1 级, 第 2 级, …, 原来的排序」排好，`nested_groups` 对每一级各做一次 `GROUP BY 前 k 级`（同样的表达式和 `NULLS LAST`），内层的组按行数依次落在外层的组里，所以前端不用知道父子关系，只拿每一级的行数就能还原嵌套。`GroupMap` 把显示行组织成一串「块」（一段数据行或一行小计，按「内层组的数据 → 内层小计 → … → 外层小计」展开，折叠的组只出它自己的小计块），块的起点做前缀和二分查找；每个最内层组记住它显示的位置，被折叠时指向显示出来的那行小计。折叠状态按「各级取值的路径」记，筛选变了也认得。导出时每组开始前按它占的行数（含里面的小计行）调用 `group_rows`，嵌套调用会叠加级别、隐藏只增不减，改写时一行的级别 = 包含它的组的个数；外层小计的 `SUBTOTAL` 区域里有内层小计，Excel 会跳过它们，只有对内层分组列「计数」时小计名字会被数进去，这种格子写算好的值。

## 汇总行

`dataset::totals`。汇总行用的是表格已经建好的游标（筛选、搜索、规则都已经物化成位置表，或是直通游标），对游标覆盖的整个结果集算，所以和表格显示的行严格一致，也自动带上数据范围过滤（游标的 `from_sql` 连接的是按用户过滤后的表）。每列一种汇总方式，所有列拼进**一条** `SELECT`（`sum` / `avg` / `median` / `min` / `max` / `count(*) FILTER (…)` / `count(DISTINCT …)`），一次扫描；可选的方式按列类型限定（`dataset::total_funcs`，前端 `big-grid/totals.ts` 与之一致），不适用时报错而不是返回无意义的值。日期列存的是文本，最早 / 最晚只在符合日期格式的值里取（与数据概况的 `DATE_RE` 一致）。

前端把汇总行画在 canvas 外的一行 HTML 里（`BigGrid` 的 `footer`），按 `scrollLeft` 平移与列对齐；切换游标（筛选变了、数据变了）就重算，改了某列的方式只等那一列。开关和每列的选择存在浏览器本地（与列宽一样），不影响别人。

## 数据校验

`server/src/validation.rs`。每条规则翻译成「不合格的行」的 SQL 条件（`violation_sql`，值都走参数）：必填是空值或只有空白；不能重复是 `列 IN (SELECT 列 … GROUP BY 列 HAVING count(*) > 1)`；范围、格式（`regexp_full_match`）、取值列表都先排除空值（空值归「必填」管）。检查把所有规则拼成一条 `SELECT count(*) FILTER (WHERE 条件1), …, count(*) FILTER (WHERE 条件1 OR 条件2 …)`，一次扫描得到每条规则和「至少违反一条」的行数，再给不合格的规则各取几个样例值。规则和最近一次结果存在 `ledger_validation`，数据集列表带一份小结。

同一个条件也是查询条件的一部分：`QuerySpec::violation` 放一条规则，`where_clause` 把它和其他筛选一起拼进 WHERE，所以「只看不合格的行」对游标、透视、导出、数据清洗都一样有效（直通游标的判断也要看它）。数据变了以后（`sqlq::on_data_changed`：导入、同步、外部刷新、清洗）在后台重新检查，有不合格的发 `dataset.validation_failed`。数据范围受限的用户手动检查时，数据集定义已经换成带过滤的子查询，结果只代表他能看到的行，所以不保存。

推荐规则（`validation::suggest`）不用模型：数据概况一次扫描（空值、只有空白、不同值、负数、认不出的日期），文本 / 编号列再一次扫描数出匹配手机号、邮箱、身份证号正则的个数（内置正则直接写进 SQL，没有用户输入），取值很少（≤ 12 种）的文本列各做一次 `GROUP BY` 拿到每种取值的次数，按阈值（九成匹配、列名像编号、空值 ≤ 5%、负数 ≤ 5%、只出现一两次且不到一成的算写错）推荐，最后把推荐的规则一起 `run` 一遍得到「现在有几行不合格」。AI 的 `propose_validation` 不给规则时用它，给了就先 `preview`（与保存相同的检查，不写库）；建议卡记下原来的规则，前端应用时调保存接口，撤销前确认规则没被别人改过再写回原来的。

## 拆分 / 合并列

拆分 / 合并不改存储的数据，而是生成计算列（`calc.rs` 的 `SPLITPART` / `TEXTAFTER` / `TEXTJOIN` / `CONCAT`，翻译成 DuckDB 的 `split_part` / `string_split` / `concat_ws`）：导入追加、数据连接同步时只写登记过来源的列，物理上拆出来的新列在新行里会是空的，而计算列对每一行（包括以后进来的）都有效，删掉就回到原样，也不用另做撤销。`column_split.rs` 只负责预览：一条 `GROUP BY len(string_split(列, 分隔符))` 数出各段数的行数，再取几个样例（最常见段数的在前），前端据此定列数、起列名，按 `features/excel/dataset/split-merge.ts` 生成公式后逐个加计算列，中途失败把已加的撤回。

## 数据清洗

`server/src/clean.rs`。每种清洗（删除重复行、去空白、查找替换、填充空值、删除筛选出的行）先由 `plan` 翻译成「哪些行」（条件）和「改成什么」（列 → 新值表达式），预览和执行共用同一份，所以预览的行数就是执行的行数。删除重复行按 `row_number() OVER (PARTITION BY 列 ORDER BY rowid) > 1` 找行，向下填充用 `last_value(… IGNORE NULLS) OVER (ORDER BY rowid …)`，删除筛选出的行复用数据表的筛选（`dataset::rows_sql`）。

执行在导入锁里进行。撤销数据放在 `clean_undo.<数据集 id>`：删除是先把要删的整行 `SELECT *` 存下来；修改是存 `rowid` 和原值，新值先算进临时表再 `UPDATE … FROM`（避免在 UPDATE 里用窗口函数）。`ledger_clean` 每个数据集只记最近一次，并记下执行后数据集的 `updated_at` 和列结构；撤销前比对这两项，之后又有改动就拒绝撤销（不去猜怎么合并）。删除的行撤销后追加在表尾，行顺序会变。改到文本列时重算 `search_text`；最后调用 `sqlq::on_data_changed`。数据集彻底删除时一起清掉撤销数据。

## 窗口计算

`server/src/window.rs`。计算列是在筛选、透视、公式下推的 SQL 里就地展开的表达式（`dataset::column_sql`），而窗口函数（`… OVER (…)`）不能出现在 WHERE 或别的聚合里，所以窗口计算不做成计算列，而是把结构化的设置（计算、分组、组内顺序、每组前 N 行）翻译成 `SELECT *, <窗口表达式> FROM 数据集 [QUALIFY …] ORDER BY 分组, 顺序`（标识符用 `schema::ident` 转义），交给 `sqlq::save` 保存为 SQL 查询并另存为数据集，计划默认是 `after_data`（源数据集更新后，见 `sqlq::on_data_changed`）。这样窗口结果是物化的普通列，所有基于数据集的功能都能用；代价是派生数据集占一份存储，更新有延迟（重算期间看到的是上一次的结果）。
