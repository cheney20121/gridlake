# REST 接口参考

> 本文件由 `scripts/gen-api-docs.py` 根据后端源码里的文档注释与权限表生成，**请不要手工编辑**；改了接口后重新运行脚本。
> 请求体 / 参数的详细字段以对应的 Rust 结构体（`server/src/` 里处理函数的参数类型）为准，下表「说明」列给出的是接口注释里的概要。

## 约定

- **基础路径**：所有接口都在 `/api` 下（页面与服务同源时即 `http://主机:8787/api/...`）。
- **响应格式**：JSON 信封 `{ "code": 200, "data": ..., "msg": "success" }`；出错时 `code` 是 HTTP 状态码、`msg` 是可以直接展示给用户的说明。文件下载、导出、事件流等接口返回原始内容。
- **状态码**：`400` 参数不合法 · `401` 未登录或登录已失效 · `403` 没有权限 · `404` 接口不存在 · `503` 服务繁忙（大计算排队超时）或身份服务不可用 · `500` 内部错误。
- **认证**（`LEDGER_AUTH_MODE` 不为 `off` 时）：
  - 脚本调用：登录取得令牌（`POST /api/ledger/auth/login`，内置账号模式），放进请求头 `x-user-token`；网关模式用网关发的令牌，同样放在 `x-user-token`；
  - 浏览器：登录时服务端写入 `HttpOnly` 会话 cookie；用 cookie 的**写请求**必须带请求头 `x-requested-with`（任意非空值），防跨站请求伪造；
  - `/api/admin/*`：请求头 `x-admin-token` 或 `Authorization: Bearer <令牌>`（未配置 `LEDGER_ADMIN_TOKEN` 时只允许本机访问）；
  - `/api/mcp`：`Authorization: Bearer <MCP 令牌>`（在页面「数据中心 → 外部 AI 接入」里创建）。
- **操作人**：写请求里的 `operator` 字段（查询参数或 JSON 顶层）会被服务端替换为登录用户，客户端传的值无效。
- **数据范围受限的用户**（管理员给用户设置了「只能看某些店铺」）：只能调用下表「受限用户」列为 ✓ 的接口，其余一律返回 403；✓ 的接口读取的数据也已按范围过滤。
- **审计**：「审计」列非空的接口会记入操作审计，列里是审计里显示的操作名。

共 216 个接口。

## 存活检查与运维

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| POST | `/api/admin/backup` | 立即备份（与导入等写操作串行） | 运维令牌 | — |  |
| POST | `/api/admin/backup/verify` | 恢复演练：用恢复时相同的方式验证最新的备份真的能用 查询参数：`source=local\|remote&full=1` | 运维令牌 | — |  |
| GET | `/api/admin/metrics` | Prometheus 文本格式的运行指标（见 metrics.rs） | 运维令牌 | — |  |
| GET | `/api/admin/status` | 运行状态：版本、数据文件大小、各数据集行数、失效的计算列 / 视图、备份 | 运维令牌 | — |  |
| GET | `/api/health` | 存活检查（负载均衡 / 容器健康检查用，不需要令牌） | 无需登录 | ✓ |  |

## 登录与账号

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| GET | `/api/ledger/accounts` | 账号列表（管理员） | 仅管理员 | — |  |
| POST | `/api/ledger/accounts` | { username, name, password, admin }（管理员） | 仅管理员 | — | 新建账号 |
| PUT | `/api/ledger/accounts/{id}` | { name?, admin?, disabled?, password? }（管理员） | 仅管理员 | — | 修改账号 |
| GET | `/api/ledger/auth/2fa` | 我的两步验证状态 | 登录用户 | ✓ |  |
| POST | `/api/ledger/auth/2fa/disable` | { password, code }  停用两步验证（要再输一次密码和动态码 / 恢复码） | 登录用户 | ✓ | 停用两步验证 |
| POST | `/api/ledger/auth/2fa/enable` | { code }  用验证器里的 6 位数字确认并启用；返回 10 个一次性恢复码（只显示这一次） | 登录用户 | ✓ | 启用两步验证 |
| POST | `/api/ledger/auth/2fa/setup` | 开始设置两步验证：返回密钥和 otpauth 地址（还没启用，要再验证一次动态码） | 登录用户 | ✓ | 开始设置两步验证 |
| POST | `/api/ledger/auth/login` | { username, password } → { token, days, user }，同时用 Set-Cookie 写入 HttpOnly 会话 cookie。 响应里的 token 供脚本 / 前后端分开部署时放进 x-user-token 请求头；浏览器页面不需要保存它 | 无需登录 | ✓ |  |
| POST | `/api/ledger/auth/logout` | 作废当前令牌，并清除会话 cookie | 登录用户 | ✓ | 退出登录 |
| GET | `/api/ledger/auth/oidc/callback` | 身份提供方登录后回到这里；成功 → 写会话 cookie 并 302 回应用，失败 → 302 到登录页带错误代码 | 无需登录 | ✓ |  |
| GET | `/api/ledger/auth/oidc/login` | → 302 到身份提供方 查询参数：`return=/路径` | 无需登录 | ✓ |  |
| POST | `/api/ledger/auth/password` | { oldPassword, newPassword } | 登录用户 | ✓ | 修改密码 |
| GET | `/api/ledger/me` | 是否要求登录与当前用户（没登录时 user 为空，由前端跳转登录页） | 无需登录 | ✓ |  |

## 操作审计

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| GET | `/api/ledger/audit` | 操作审计（仅管理员，由路由表限制） 查询参数：`user=&action=&from=&to=&failed=&page=&size=` | 仅管理员 | — |  |

## 工作簿、版本与共享

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| GET | `/api/ledger/folders` |  | 登录用户 | ✓ |  |
| POST | `/api/ledger/folders` | { name, parentId, operator } | 登录用户 | ✓ | 新建文件夹 |
| PUT | `/api/ledger/folders/{id}` | { name, parentId }  改名 / 移动 | 登录用户 | ✓ | 修改文件夹 |
| DELETE | `/api/ledger/folders/{id}` | 删除文件夹（含子文件夹），其中的工作簿移到回收站 查询参数：`operator=` | 登录用户 | ✓ | 删除文件夹 |
| GET | `/api/ledger/workbooks` |  | 登录用户 | ✓ |  |
| POST | `/api/ledger/workbooks` | body: gzip 压缩的工作簿 JSON 查询参数：`name=&operator=` | 登录用户 | ✓ | 新建工作簿 |
| POST | `/api/ledger/workbooks/move` | { ids, folderId }  移到文件夹（folderId 为空表示根目录） | 登录用户 | ✓ | 移动工作簿 |
| POST | `/api/ledger/workbooks/ranges` | { items: [{ workbook, range }] }  跨工作簿引用：取其他工作簿已保存版本里的区域值 | 登录用户 | ✓ |  |
| GET | `/api/ledger/workbooks/trash` | 回收站 | 登录用户 | ✓ |  |
| DELETE | `/api/ledger/workbooks/trash` | 清空回收站（不可恢复） | 仅管理员 | — | 清空工作簿回收站 |
| GET | `/api/ledger/workbooks/{id}` | 元数据 | 工作簿查看权限 | ✓ |  |
| PUT | `/api/ledger/workbooks/{id}` | { name }  改名 | 工作簿编辑权限 | ✓ | 重命名工作簿 |
| DELETE | `/api/ledger/workbooks/{id}` | 移到回收站（可恢复） 查询参数：`operator=` | 工作簿管理（所有者或管理员）权限 | ✓ | 删除工作簿 |
| GET | `/api/ledger/workbooks/{id}/comments` |  | 工作簿查看权限 | ✓ |  |
| POST | `/api/ledger/workbooks/{id}/comments` | { sheetId, row, col, body, mentions? } 或 { parentId, body, mentions? }（回复） | 工作簿查看权限 | ✓ | 添加评论 |
| GET | `/api/ledger/workbooks/{id}/content` | 工作簿 JSON（Content-Encoding: gzip，X-Workbook-Version 为版本号） | 工作簿查看权限 | ✓ |  |
| PUT | `/api/ledger/workbooks/{id}/content` | body: gzip 压缩的 JSON 查询参数：`baseVersion=&force=&auto=&description=&operator=` 冲突时返回 saved: false + conflict（不覆盖） | 工作簿编辑权限 | ✓ | 保存工作簿 |
| POST | `/api/ledger/workbooks/{id}/duplicate` | { name, operator }  复制为新工作簿 | 工作簿查看权限 | ✓ | 复制工作簿 |
| DELETE | `/api/ledger/workbooks/{id}/purge` | 彻底删除（只能删回收站里的，含全部版本） | 工作簿管理（所有者或管理员）权限（含回收站） | ✓ | 彻底删除工作簿 |
| POST | `/api/ledger/workbooks/{id}/restore` | { version, operator }  以历史版本内容生成新版本 | 工作簿编辑权限 | ✓ | 恢复工作簿历史版本 |
| PUT | `/api/ledger/workbooks/{id}/share` | { visibility: private \| view \| edit }  修改共享范围（所有者与管理员） | 工作簿管理（所有者或管理员）权限 | ✓ | 修改工作簿共享范围 |
| GET | `/api/ledger/workbooks/{id}/shares` | 这个工作簿的分享链接（所有者与管理员） | 工作簿管理（所有者或管理员）权限 | ✗ |  |
| POST | `/api/ledger/workbooks/{id}/shares` | { note?, days?, password? }  新建分享链接；响应里的 token 只显示这一次 | 工作簿管理（所有者或管理员）权限 | ✗ | 创建分享链接 |
| PUT | `/api/ledger/workbooks/{id}/star` | { starred } | 工作簿查看权限 | ✓ |  |
| POST | `/api/ledger/workbooks/{id}/untrash` | 从回收站恢复 | 工作簿管理（所有者或管理员）权限（含回收站） | ✓ | 恢复工作簿 |
| GET | `/api/ledger/workbooks/{id}/versions` |  | 工作簿查看权限 | ✓ |  |
| GET | `/api/ledger/workbooks/{id}/versions/{version}/content` |  | 工作簿查看权限 | ✓ |  |

## 数据集（导入、取数、透视、公式下推）

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| GET | `/api/ledger/datasets` | 数据集与列定义 | 登录用户 | ✓ |  |
| POST | `/api/ledger/datasets/external` | { name, path, pattern, recursive, hive }  新建外部数据集（只读，不导入） | 仅管理员 | — | 新建外部数据集 |
| POST | `/api/ledger/datasets/external/preview` | { path, pattern, recursive, hive }  预览外部 Parquet 文件：匹配的文件、总行数、列与类型、前几行 | 仅管理员 | — |  |
| POST | `/api/ledger/datasets/import/commit` | { token, mode: create \| append \| replace, datasetId?, name?, columns, primaryKey, fileName } | 登录用户 | ✗ | 导入数据集 |
| POST | `/api/ledger/datasets/import/preview` | multipart: file（CSV / XLS / XLSX） | 登录用户 | ✗ |  |
| POST | `/api/ledger/datasets/import/preview-url` | { url }  从网址下载文件并预览（http / https，不访问本机地址） | 登录用户 | ✗ |  |
| POST | `/api/ledger/datasets/input` | { name, columns: [{ title, kind }], primaryKey }  新建空的填报数据集，返回 id | 登录用户 | ✗ | 新建填报数据集 |
| POST | `/api/ledger/datasets/summaries` | { name, def: { source, rows, values, filters?, keyword? } }  透视表转为数据表：新建汇总数据集，返回数据集 id | 登录用户 | ✗ |  |
| GET | `/api/ledger/datasets/trash` | 数据集回收站 { days, items } | 登录用户 | ✗ |  |
| DELETE | `/api/ledger/datasets/trash` | 清空数据集回收站（不可恢复） | 仅管理员 | — | 清空数据集回收站 |
| POST | `/api/ledger/datasets/views` | { name, left, right, on, columns }  创建关联视图 | 登录用户 | ✗ | 创建关联视图 |
| POST | `/api/ledger/datasets/views/preview` | { left, right, on: [{ left, right }], columns: [{ source, title }] }  关联预览（匹配率、关联不上的键） | 登录用户 | ✗ |  |
| PUT | `/api/ledger/datasets/views/{id}` | { name, left, right, on, columns }  修改关联视图 | 数据集所有者或管理员 | ✗ | 修改关联视图 |
| PUT | `/api/ledger/datasets/{id}` | { name }  改名 | 数据集所有者或管理员 | ✗ | 重命名数据集 |
| DELETE | `/api/ledger/datasets/{id}` | 删除数据集：移到回收站（数据保留，可恢复） 查询参数：`operator=` | 数据集所有者或管理员 | ✗ | 删除数据集 |
| POST | `/api/ledger/datasets/{id}/aggregate` | { calls: [{ func, column, criteria: [{ column, value }], row? }], query? }  公式下推批量计算 | 登录用户 | ✓ |  |
| GET | `/api/ledger/datasets/{id}/anomalies` | 异常发现：最近 7 天相对此前 28 天明显偏离的走势与分组（受限用户只扫描自己范围内的数据） | 登录用户 | ✓ |  |
| POST | `/api/ledger/datasets/{id}/calc` | { name, formula, kind? }  新增计算列 | 数据集所有者或管理员 | ✗ | 新增计算列 |
| POST | `/api/ledger/datasets/{id}/calc/preview` | { formula }  计算列预览（类型、样例、空值行数） | 登录用户 | ✓ |  |
| PUT | `/api/ledger/datasets/{id}/calc/{name}` | { formula, kind? }  修改计算列 | 数据集所有者或管理员 | ✗ | 修改计算列 |
| DELETE | `/api/ledger/datasets/{id}/calc/{name}` |  | 数据集所有者或管理员 | ✗ | 删除计算列 |
| POST | `/api/ledger/datasets/{id}/cursor` | QuerySpec → 游标（筛选 + 排序物化，按条件缓存） | 登录用户 | ✓ |  |
| POST | `/api/ledger/datasets/{id}/distinct` | 列值分面（筛选下拉） | 登录用户 | ✓ |  |
| GET | `/api/ledger/datasets/{id}/export` | 按当前筛选 / 排序导出（流式下载） 查询参数：`cursor=&columns=&name=&format=csv\|xlsx&desc=` | 登录用户 | ✓ | 导出数据集 |
| POST | `/api/ledger/datasets/{id}/external/refresh` | 重新查找文件、读取结构（文件有增减或结构变化后） | 仅管理员 | — | 刷新外部数据集 |
| POST | `/api/ledger/datasets/{id}/pivot` | 数据透视表（rows / columns / values / filters / sort / top / subtotals / grandTotals） | 登录用户 | ✓ |  |
| DELETE | `/api/ledger/datasets/{id}/purge` | 彻底删除（连同回收站里基于它的关联视图），返回删除的名称 | 数据集所有者或管理员 | ✗ | 彻底删除数据集 |
| GET | `/api/ledger/datasets/{id}/rows` | 按位置取一段 查询参数：`cursor=&offset=&limit=&columns=` | 登录用户 | ✓ |  |
| POST | `/api/ledger/datasets/{id}/stats` | 选区统计 | 登录用户 | ✓ |  |
| POST | `/api/ledger/datasets/{id}/untrash` | 从回收站恢复 | 数据集所有者或管理员 | ✗ | 恢复数据集 |
| GET | `/api/ledger/datasets/{id}/usage` | 使用情况：引用它的工作簿 / 模板 / 视图 / SQL / 连接 / 推送，近 30 天访问次数 | 登录用户 | ✗ |  |
| POST | `/api/ledger/datasets/{id}/writeback` | { mode: upsert\|append, columns, rows, keys?, operator?, source? } 整批校验，有错误时不写入并返回出错的格子（errors） | 登录用户 | ✓ | 填报提交 |
| GET | `/api/ledger/writebacks` | 提交记录（新的在前），附带能否撤销 查询参数：`datasetId=&limit=` | 登录用户 | ✗ |  |
| POST | `/api/ledger/writebacks/{id}/undo` | 撤销一次提交（数据集之后没有别的改动时） 查询参数：`operator=` | 登录用户 | ✗ | 撤销填报提交 |

## SQL 工作台

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| GET | `/api/ledger/sql/queries` | 已保存的查询（关联数据集、最近一次重算、下次重算时间） | 登录用户 | ✗ |  |
| POST | `/api/ledger/sql/queries` | { name, sql, description?, saveAsDataset?, columnKinds?, schedule?, operator? } | 登录用户 | ✗ | 保存 SQL 查询 |
| PUT | `/api/ledger/sql/queries/{id}` | 修改（已关联数据集时立即按新 SQL 重算） | 登录用户 | ✗ | 修改 SQL 查询 |
| DELETE | `/api/ledger/sql/queries/{id}` | 删除查询（另存的数据集保留） | 登录用户 | ✗ | 删除 SQL 查询 |
| POST | `/api/ledger/sql/queries/{id}/instances` | { values, name?, operator? }  用一组参数值生成实例（独立的查询 + 新数据集） | 登录用户 | ✗ | 生成参数化查询的实例 |
| POST | `/api/ledger/sql/queries/{id}/refresh` | { datasetName? }  立即重算并写入数据集 | 登录用户 | ✗ | 重算 SQL 查询 |
| POST | `/api/ledger/sql/run` | { sql, limit?, params?, values? }  执行只读查询（最多返回 1000 行） | 登录用户 | ✓ | 执行 SQL 查询 |

## 逐笔对账

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| POST | `/api/ledger/recons` | { name, def }  创建对账（同时得到一个同名的结果数据集），返回数据集 id | 登录用户 | ✓ | 创建对账 |
| POST | `/api/ledger/recons/preview` | { def }  新建前预览：两侧行数、日期范围、容易填错的地方 | 登录用户 | ✓ |  |
| GET | `/api/ledger/recons/{id}` | 对账的定义 | 登录用户 | ✓ |  |
| PUT | `/api/ledger/recons/{id}` | { name, def, resetMatches? }  修改（所有者或管理员） | 数据集所有者或管理员 | ✓ | 修改对账 |
| POST | `/api/ledger/recons/{id}/auto` | { dryRun? }  自动勾对（dryRun 只试算不保存） | 登录用户 | ✓ | 自动勾对 |
| GET | `/api/ledger/recons/{id}/groups` | 勾对组（含组内的行） 查询参数：`method=&withDiff=&broken=&q=&offset=&limit=` | 登录用户 | ✓ |  |
| GET | `/api/ledger/recons/{id}/items` | 行列表 查询参数：`side=left\|right&status=open\|matched\|all&q=&amount=&from=&to=&offset=&limit=` | 登录用户 | ✓ |  |
| POST | `/api/ledger/recons/{id}/match` | { left: [行标识], right: [行标识], note?, force? }  手工勾对（任意多对多） | 登录用户 | ✓ | 手工勾对 |
| GET | `/api/ledger/recons/{id}/summary` | 汇总：两侧笔数与金额、截至日的未达账项、勾对组统计 查询参数：`asOf=` | 登录用户 | ✓ |  |
| POST | `/api/ledger/recons/{id}/unmatch` | { groups: [勾对组] } 或 { mode: auto \| all }  取消勾对 | 登录用户 | ✓ | 取消勾对 |

## 数据连接

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| GET | `/api/ledger/connectors` | 连接器列表（含最近一次同步、下次同步时间） | 登录用户 | ✗ |  |
| POST | `/api/ledger/connectors` | 新建 | 仅管理员 | — | 新建数据连接 |
| POST | `/api/ledger/connectors/preview` | 预览来源数据（样例行、推断的列） | 仅管理员 | — |  |
| POST | `/api/ledger/connectors/test` | 测试来源（数据库连通性 / 文件夹匹配的文件） | 仅管理员 | — |  |
| PUT | `/api/ledger/connectors/{id}` | 修改（密码留空则沿用已保存的） | 仅管理员 | — | 修改数据连接 |
| DELETE | `/api/ledger/connectors/{id}` | 删除连接器（数据集保留） | 仅管理员 | — | 删除数据连接 |
| POST | `/api/ledger/connectors/{id}/run` | 立即同步（后台执行，返回同步记录 ID） | 仅管理员 | — | 手动同步数据连接 |
| GET | `/api/ledger/connectors/{id}/runs` | 同步记录（新的在前） 查询参数：`limit=` | 登录用户 | ✗ |  |

## 定时推送

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| POST | `/api/ledger/report-channels` | 新建推送渠道 | 仅管理员 | — | 新建推送渠道 |
| PUT | `/api/ledger/report-channels/{id}` | 修改（地址留空沿用已保存的） | 仅管理员 | — | 修改推送渠道 |
| DELETE | `/api/ledger/report-channels/{id}` |  | 仅管理员 | — | 删除推送渠道 |
| POST | `/api/ledger/report-channels/{id}/test` | 发送测试消息 | 仅管理员 | — | 测试推送渠道 |
| GET | `/api/ledger/reports` | 推送任务、渠道（地址已打码）、邮件是否已配置、看板链接前缀 | 登录用户 | ✗ |  |
| POST | `/api/ledger/reports` | 新建推送任务 | 所指工作簿的查看权限 | ✗ | 新建定时推送 |
| POST | `/api/ledger/reports/preview` | { workbookId, sheetId, title? }  预览推送内容 | 所指工作簿的查看权限 | ✗ |  |
| GET | `/api/ledger/reports/sheets` | 工作簿里含仪表盘部件的工作表 查询参数：`workbookId=` | 所指工作簿的查看权限 | ✓ |  |
| PUT | `/api/ledger/reports/{id}` |  | 登录用户 | ✗ | 修改定时推送 |
| DELETE | `/api/ledger/reports/{id}` |  | 登录用户 | ✗ | 删除定时推送 |
| GET | `/api/ledger/reports/{id}/runs` | 推送记录（新的在前） 查询参数：`limit=` | 登录用户 | ✗ |  |
| POST | `/api/ledger/reports/{id}/send` | 立即发送（等待完成，返回本次记录） | 登录用户 | ✗ | 立即发送推送 |

## 模板

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| GET | `/api/ledger/templates` | 内置与自建模板（需要的数据集与列、概要、使用次数） | 登录用户 | ✓ |  |
| POST | `/api/ledger/templates` | { workbookId, name, description, category, replaceId?, operator? }  把已保存的工作簿发布为模板 | 所指工作簿的查看权限 | ✗ | 发布模板 |
| PUT | `/api/ledger/templates/{id}` | { name, description, category } | 登录用户 | ✗ | 修改模板 |
| DELETE | `/api/ledger/templates/{id}` | 删除自建模板（已用它创建的工作簿不受影响） | 登录用户 | ✗ | 删除模板 |
| POST | `/api/ledger/templates/{id}/use` | { name, bindings: { key: { datasetId, columns } }, clearSlicers?, operator? }  用模板创建工作簿 | 登录用户 | ✓ | 使用模板创建工作簿 |
| GET | `/api/ledger/templates/{id}/uses` | 最近 50 次使用（生成的工作簿） | 登录用户 | ✓ |  |

## AI 助手

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| POST | `/api/ledger/ai/actions/{id}` | { status: applied \| dismissed, result?, operator? }  记录建议的处理结果 | 所属会话的创建人或管理员 | ✓ | 处理 AI 建议 |
| GET | `/api/ledger/ai/status` | 是否已配置模型（模型名 @ 主机，不含密钥） | 登录用户 | ✓ |  |
| GET | `/api/ledger/ai/threads` | 工作簿的会话（新的在前） 查询参数：`workbookId=` | 所指工作簿的查看权限 | ✓ |  |
| POST | `/api/ledger/ai/threads` | { workbookId, operator? }  新会话 | 所指工作簿的查看权限 | ✓ | 新建 AI 会话 |
| GET | `/api/ledger/ai/threads/{id}` | 会话内容：提问、回答、工具调用、建议及处理结果 | 会话创建人或管理员 | ✓ |  |
| DELETE | `/api/ledger/ai/threads/{id}` |  | 会话创建人或管理员 | ✓ | 删除 AI 会话 |
| POST | `/api/ledger/ai/threads/{id}/chat` | { message, context } 或 { resume: true }（接着回答被中断的问题）  提问；以 SSE 返回事件： start / status{text}（如整理摘要）/ summary{summary} / text{delta} / break / tool{id,name,status,summary,actionId} / action{action} / done / error{message} | 会话创建人或管理员 | ✓ | AI 提问 |
| GET | `/api/ledger/ai/threads/{id}/events` | 重新连上正在进行的回答：先补发本次回答已产生的事件，再继续推送； 没有在回答时只返回 { type: idle } | 会话创建人或管理员 | ✓ |  |
| POST | `/api/ledger/ai/threads/{id}/stop` | 停止正在进行的回答（已产生的文字保留） | 会话创建人或管理员 | ✓ |  |
| PUT | `/api/ledger/ai/threads/{id}/summary` | { summary }  手动编辑会话摘要；留空 = 清除，之后提问时重新整理 | 会话创建人或管理员 | ✓ | 编辑 AI 会话摘要 |

## 外部 AI 接入（MCP）

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| GET | `/api/ledger/mcp/logs` | 调用审计日志（新的在前） 查询参数：`limit=` | 仅管理员 | — |  |
| GET | `/api/ledger/mcp/tokens` | 有效的 MCP 令牌（不含明文） | 仅管理员 | — |  |
| POST | `/api/ledger/mcp/tokens` | { name, allowSql?, datasets?, operator? } → { token, secret }（明文只返回这一次） | 仅管理员 | — | 创建外部 AI 令牌 |
| PUT | `/api/ledger/mcp/tokens/{id}` | 修改名称 / 是否允许 SQL / 可访问的数据集 | 仅管理员 | — | 修改外部 AI 令牌 |
| DELETE | `/api/ledger/mcp/tokens/{id}` | 吊销（立即失效） | 仅管理员 | — | 吊销外部 AI 令牌 |
| GET | `/api/mcp` | 405：本服务不提供服务端推送流（Streamable HTTP 的可选部分） | MCP 令牌 | — |  |
| POST | `/api/mcp` | MCP Streamable HTTP：JSON-RPC 2.0（单条或批量），需 Authorization: Bearer <令牌> | MCP 令牌 | — |  |
| DELETE | `/api/mcp` | 405：本服务不维护会话 | MCP 令牌 | — |  |

## 评论、协作与通知

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| GET | `/api/ledger/collab/{id}/events` | 加入房间，SSE 推送：hello / join / leave / presence / ops / saved / replaced / request-save / stale 查询参数：`client=&version=&name=` | 工作簿查看权限 | ✓ |  |
| POST | `/api/ledger/collab/{id}/send` | { client, kind, batchId?, ops?, presence? } | 工作簿查看权限 | ✓ |  |
| PUT | `/api/ledger/comments/{id}` | { body, mentions? } | 登录用户 | ✓ | 修改评论 |
| DELETE | `/api/ledger/comments/{id}` |  | 登录用户 | ✓ | 删除评论 |
| POST | `/api/ledger/comments/{id}/resolve` | { resolved } | 登录用户 | ✓ | 标记评论线程已解决 |
| GET | `/api/ledger/notifications` | 我的通知（新的在前）与未读数 | 登录用户 | ✓ |  |
| POST | `/api/ledger/notifications/read` | { ids? }  标记已读；不带 ids 表示全部 | 登录用户 | ✓ |  |

## 审批

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| GET | `/api/ledger/approvals` | 查询参数：`box=todo\|started\|done\|all` | 登录用户 | ✓ |  |
| POST | `/api/ledger/approvals` | { workbookId, approverId, comment, summary }  发起审批（审批的是工作簿当前已保存的版本） | 所指工作簿的查看权限 | ✓ | 发起文件审批 |
| GET | `/api/ledger/approvals/count` | { todo } | 登录用户 | ✓ |  |
| GET | `/api/ledger/approvals/{id}` | 详情（含历史） | 登录用户 | ✓ |  |
| POST | `/api/ledger/approvals/{id}/cancel` | { comment } | 登录用户 | ✓ | 撤回文件审批 |
| POST | `/api/ledger/approvals/{id}/decide` | { approve, comment } | 登录用户 | ✓ | 处理文件审批 |
| GET | `/api/ledger/users` | 登录过的用户（选审批人） 查询参数：`keyword=` | 登录用户 | ✓ |  |

## 数据范围（行级权限）

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| GET | `/api/ledger/data-scope` | 规则、数据集绑定、维度与取值候选 | 仅管理员 | — |  |
| PUT | `/api/ledger/data-scope/datasets/{id}` | { scopeColumn?, scopeDimension?, open } | 仅管理员 | — | 设置数据集的范围绑定 |
| PUT | `/api/ledger/data-scope/rules` | { subjectId, subjectName?, dimension?, mode: all \| values, values }  subjectId 为 * 是默认规则；dimension 不写按「店铺」 | 仅管理员 | — | 设置用户数据范围 |
| DELETE | `/api/ledger/data-scope/rules/{id}` | 删除规则（该用户回到默认规则）；不写 dimension 删掉这个用户的全部规则 查询参数：`dimension=` | 仅管理员 | — | 删除用户数据范围 |

## 月结与资金台账（电商场景）

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| GET | `/api/ledger/analysis/breakdown` | 查询参数：`period=&shop=&by=channel\|talent\|product\|order_month\|shop\|entity` | 登录用户 | ✗ |  |
| GET | `/api/ledger/analysis/cross-period` | 下单月份 × 结算月份 查询参数：`shop=` | 登录用户 | ✗ |  |
| GET | `/api/ledger/analysis/qianchuan` | 千川充值 / 消耗 / 千川渠道成交 查询参数：`period=` | 登录用户 | ✗ |  |
| PUT | `/api/ledger/analysis/qianchuan` | { shop, period, spend, remark, operator }  录入千川消耗 | 登录用户 | ✗ | 录入千川消耗 |
| GET | `/api/ledger/balances` | 余额勾稽 查询参数：`period=&shop=` | 登录用户 | ✗ |  |
| PUT | `/api/ledger/balances` | { operator, items: [{ shop, entity, account, period, opening, closing, remark }] } | 登录用户 | ✗ | 保存余额 |
| GET | `/api/ledger/bank/imports` | 已导入的银行流水批次 | 登录用户 | ✗ |  |
| POST | `/api/ledger/bank/imports` | multipart: file + entity (+ accountNo)  银行流水导入 | 登录用户 | ✗ | 导入银行流水 |
| DELETE | `/api/ledger/bank/imports` | 查询参数：`batchId=` | 登录用户 | ✗ | 删除银行流水批次 |
| GET | `/api/ledger/close` | 各店铺月结状态与体检 查询参数：`period=` | 登录用户 | ✓ |  |
| POST | `/api/ledger/close` | { shop, period, action: submit \| approve \| reject \| cancel \| reopen, operator, approver?, comment, acceptRisk? } | 登录用户 | ✓ | 月结操作 |
| GET | `/api/ledger/close/locks` | 已冻结（审批中 / 已锁账）的店铺月份 | 登录用户 | ✓ |  |
| GET | `/api/ledger/close/logs` | 查询参数：`period=` | 登录用户 | ✓ |  |
| GET | `/api/ledger/coverage` | 店铺 × 月份完整性矩阵 | 登录用户 | ✗ |  |
| DELETE | `/api/ledger/data` | 查询参数：`shop=&period=` | 仅管理员 | — | 删除店铺流水数据 |
| POST | `/api/ledger/details/export` | 当前筛选结果导出为 CSV | 登录用户 | ✗ | 导出流水明细 |
| POST | `/api/ledger/details/rows` | 按偏移量取一段明细（前端虚拟滚动按可视区域分段加载） | 登录用户 | ✗ |  |
| POST | `/api/ledger/details/summary` | 当前筛选的笔数、金额合计、规则贡献合计与分面统计 | 登录用户 | ✗ |  |
| GET | `/api/ledger/entity-aliases` |  | 登录用户 | ✗ |  |
| PUT | `/api/ledger/entity-aliases` | body: [{ entity, alias }]，alias 为空表示删除 | 登录用户 | ✗ | 保存主体简称 |
| GET | `/api/ledger/imports` |  | 登录用户 | ✗ |  |
| POST | `/api/ledger/imports` | multipart: shop + file (+ mode) —— 一步导入（脚本/命令行使用），默认按流水号合并 | 登录用户 | ✗ | 导入流水明细 |
| POST | `/api/ledger/imports/commit` | { token, shop, mode: merge \| replace } | 登录用户 | ✗ | 导入店铺流水 |
| POST | `/api/ledger/imports/preview` | multipart: file —— 暂存文件，识别店铺并查重，返回 token | 登录用户 | ✗ |  |
| GET | `/api/ledger/periods` |  | 登录用户 | ✓ |  |
| GET | `/api/ledger/pivot` | 查询参数：`period=2026-09` | 登录用户 | ✗ |  |
| GET | `/api/ledger/shops` |  | 登录用户 | ✓ |  |
| POST | `/api/ledger/shops` | { name, douyinShopId?, remark? } | 登录用户 | ✗ | 新增店铺 |
| PUT | `/api/ledger/shops` | { name, newName?, douyinShopId?, status?, remark? }（改名同步更新历史数据） | 登录用户 | ✗ | 修改店铺 |
| GET | `/api/ledger/withdrawals` | 提现到账情况 查询参数：`period=&shop=` | 登录用户 | ✗ |  |
| POST | `/api/ledger/withdrawals/auto-match` | { period, operator } | 登录用户 | ✗ | 提现自动配对 |
| GET | `/api/ledger/withdrawals/candidates` | 手工配对候选 查询参数：`shop=&period=&serialNo=` | 登录用户 | ✗ |  |
| PUT | `/api/ledger/withdrawals/match` | { shop, period, serialNo, txnId, operator }（txnId 为空 = 取消配对） | 登录用户 | ✗ | 提现手动配对 |

## 其他

| 方法 | 路径 | 说明 | 权限 | 受限用户 | 审计 |
|---|---|---|---|---|---|
| GET | `/api/ledger/alerts` | 告警规则、可选的推送渠道（地址已打码）、邮件是否已配置 | 登录用户 | ✗ |  |
| POST | `/api/ledger/alerts` | 新建告警（受数据范围限制的用户不能建） | 登录用户 | ✗ | 新建告警 |
| POST | `/api/ledger/alerts/preview` | 试算：按当前数据算一遍会命中哪些分组，不保存、不发送 | 登录用户 | ✗ |  |
| PUT | `/api/ledger/alerts/{id}` | 只有创建人或管理员能改 | 登录用户 | ✗ | 修改告警 |
| DELETE | `/api/ledger/alerts/{id}` | 删除告警及其评估记录（创建人或管理员） | 登录用户 | ✗ | 删除告警 |
| POST | `/api/ledger/alerts/{id}/evaluate` | 立即评估一次并按规则通知（等待完成，返回本次记录） | 登录用户 | ✗ | 立即评估告警 |
| GET | `/api/ledger/alerts/{id}/runs` | 评估记录（新的在前） 查询参数：`limit=` | 登录用户 | ✗ |  |
| GET | `/api/ledger/collects` | 收集任务列表（含最近一期的提交进度） | 仅管理员 | — |  |
| POST | `/api/ledger/collects` | 新建任务：{ name, datasetId \| newTable, unitColumn?, periodColumn?, dimension?, units: [{ unit, userId? }] } 单位与负责人会同步成数据权限（负责人只能看 / 填自己负责的单位）。新建填报表时数据集建在这里，所以和导入一样串行 | 仅管理员 | — | 新建收集任务 |
| GET | `/api/ledger/collects/mine` | 我负责的单位在最近几期里填了没有（受数据范围限制的用户也能看） | 登录用户 | ✓ |  |
| GET | `/api/ledger/collects/{id}` | 看板：已开启的期次，以及某一期（默认最近一期）每个单位的状态。管理员和这个任务的确认人能看 查询参数：`period=` | 登录用户 | ✓ |  |
| PUT | `/api/ledger/collects/{id}` | { name, units }  改名、整体替换填报单位与负责人 | 仅管理员 | — | 修改收集任务 |
| DELETE | `/api/ledger/collects/{id}` | 删除任务及其期次、提交记录；数据集和里面的数据保留 | 仅管理员 | — | 删除收集任务 |
| POST | `/api/ledger/collects/{id}/periods` | { period, deadline? }  开启新一期（如 2026-10），可以同时设截止日期 | 仅管理员 | — | 开启收集期次 |
| PUT | `/api/ledger/collects/{id}/periods/{period}` | { deadline }  设置 / 修改 / 取消这一期的截止日期 | 仅管理员 | — | 设置收集期次截止日期 |
| DELETE | `/api/ledger/collects/{id}/periods/{period}` | 删除还没有人提交过的一期 | 仅管理员 | — | 删除收集期次 |
| POST | `/api/ledger/collects/{id}/remind` | { period, units? }  催办：给没提交的单位的负责人发站内通知，任务配了渠道的还把名单推到群 / 邮箱。 管理员或这个任务的确认人可以催；同一个单位 1 小时内不重复催 | 登录用户 | ✓ | 催办收集填报 |
| POST | `/api/ledger/collects/{id}/review` | { period, units: [..], action: confirm \| return \| reopen, reason? } 确认人（或管理员）确认并锁定 / 退回重填（要写原因）/ 撤销确认；一次要么全部成功要么都不变 | 登录用户 | ✓ | 审核填报（确认 / 退回） |
| POST | `/api/ledger/collects/{id}/workbook` | { unit, period }  为我负责的单位生成这一期的填报工作簿（已经生成过的直接返回）； 里面放好了填报区，单位列和期次列作为固定列自动带上。管理员可以为任何单位生成 | 登录用户 | ✓ | 生成填报工作簿 |
| POST | `/api/ledger/share/unlock` | { token, password } → { proof }  访客输入访问密码（不需要登录） | 无需登录 | ✓ |  |
| DELETE | `/api/ledger/shares/{id}` | 吊销分享链接（立即生效；工作簿的所有者或管理员） | 登录用户 | ✗ | 吊销分享链接 |
| GET | `/api/ledger/tokens` | 我的访问令牌（不含令牌本身） | 登录用户 | ✓ |  |
| POST | `/api/ledger/tokens` | { name, days?, readOnly?, datasets? }  新建访问令牌；响应里的 token 只显示这一次 | 登录用户 | ✓ | 创建访问令牌 |
| DELETE | `/api/ledger/tokens/{id}` | 吊销我的访问令牌（立即生效） | 登录用户 | ✓ | 吊销访问令牌 |
