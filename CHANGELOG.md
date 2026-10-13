# 更新日志

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [未发布]

### 新增
- **多人填报（收集任务，第 1 阶段）**：首页「自动化 → 收集任务」（管理员）。让很多人往同一个数据集里各填各的：建任务时新建一张填报表（或选已有数据集，主键须含单位列和期次列），列出**填报单位**并指派**负责人**；管理员**开启期次**（如 2026-10）。单位和负责人自动同步成数据权限（数据集绑定到单位列，负责人在「填报单位」维度上只能看 / 填自己负责的单位；同一维度下的任务合并计算）。填报提交时服务端校验：填报区必须含单位列和期次列、期次必须已开启、单位必须在任务里；成功后记下谁提交了哪个单位。看板按单位 × 期次显示已提交 / 未提交、提交人、已填行数，可复制未提交名单；被指派的人在「我的填报」里看自己负责的单位（受限用户也能打开）。**被指派的负责人会成为数据范围受限用户**（导入、建视图、定时推送、告警等功能关闭），未被指派的用户不受限。新增 `ledger_collect*` 四张表（纯新增）、8 个接口、端到端测试 `test_38_collect.py`；核心只在填报路由里多两个检查点。**未做**：一键生成填报工作簿、退回重填、确认后锁定、截止时间与催办，见 ROADMAP #27。
- **阈值告警**：首页「自动化 → 阈值告警」。监控一个指标（汇总值，或两个汇总值的比率如 退款额 / 成交额），可按某一列分组逐组判断（如每家店铺），超过阈值就通知到定时推送里配置的渠道（企业微信 / 钉钉 / 飞书 / 邮件）。**同一个问题只在新出现时通知**：服务端记录处于「触发中」的分组，持续超标默认不重复（可选每次都通知或每隔 N 小时提醒），恢复后再超标重新通知；所有渠道都发送失败时不算已通知，下次重试。**最小样本量**让样本太小的分组不下结论（2 单退 1 单不会按 50% 触发）；时间窗口（昨天 / 近 7 天 / 近 30 天 / 本月 / 上月）默认不含今天；检查时间可选每天 / 每周 / 每月 / 数据同步后（在 SQL 定时重算之后）；保存前可「试算」。数据范围受限的用户不能创建、查看或试算告警（告警按服务账号读数据）；只有创建人和管理员能改。新增 `ledger_alert*` 三张表（纯新增，不升级数据文件结构版本）、6 个接口、端到端测试 `test_37_alerts.py`。
- **场景包（通用化改造）**：电商资金台账（导入抖店资金流水、店铺档案、月结锁账与审批、余额勾稽、提现 / 银行对账、经营分析、内置「资金流水」数据集和电商专用模板）从核心里拆出来，成为可选的场景包：后端 `server/src/ecommerce/`、前端 `web/src/packs/ecommerce/`，开关 `LEDGER_PACKS`。**不设置时自动判断：库里已有台账数据就启用（老部署升级后功能不消失），新库不启用**——新装的实例是纯粹的通用表格平台。没启用时对应的接口返回 404、界面里没有入口。通用的表格文件读取等工具抽到核心（`tabular.rs`）。内置模板分类去掉「电商」字样的通用模板（应收应付、银行余额调节表、库存进销存）保留。数据权限入口的文案改为按任意列取值（店铺只是例子）。
- **个人访问令牌**（内置账号登录方式）：用户菜单 →「访问令牌」创建 `glp_…` 令牌给脚本、CI 长期调用接口用（`Authorization: Bearer` 或 `x-user-token`）；权限与账号相同，可设有效期、随时吊销，库里只存摘要，创建时只显示一次；不能用令牌管理令牌。
- **英文界面**（部分）：顶栏语言切换器（中文 / English，记住选择，`?lang=en-US` 也可指定），默认跟随浏览器语言；约 2000 条界面文案有英文，Element Plus 与表格内核（Univer）的界面同步切换。写法见 CONTRIBUTING；`pnpm i18n:check` 检查漏译并进入 CI。后端错误信息也可按语言返回英文（约 700 条模板，`x-lang` 请求头或 cookie `gridlake-lang`；脚本调用默认仍是中文，`scripts/extract-messages.py` 检查漏译并进入 CI）。电商台账场景与部分运行时提示仍是中文。
- SSRF 防护：网址导入和定时推送的 Webhook 统一经 `netguard.rs` 检查。修复了几处绕过：IPv4 映射的 IPv6 写法（`::ffff:127.0.0.1`）、NAT64、阿里云元数据地址、推送 Webhook 发送时跟随重定向到本机、DNS 重绑定；新增 `LEDGER_URL_BLOCK_PRIVATE`。
- 端到端测试公共库：测试文件忘记在末尾调用 `finish()` 时，有失败项也会以失败退出。
- 社区文件：CONTRIBUTING、SECURITY、CODE_OF_CONDUCT、Issue / PR 模板。
- 发布流水线：推送 `v*` 标签自动构建 Linux x86_64 / aarch64、macOS arm64 安装包并发布 GitHub Release，同时推送多架构镜像到 GHCR；`scripts/check-version.sh` 校验版本一致；`scripts/set-repo.sh` 替换文档里的仓库占位符。
- README：截图、一分钟体验、英文简介（`README.en.md`）；`scripts/seed-demo.py` 一键导入示例订单数据。
- 开源合规：`NOTICE`、`THIRD_PARTY_LICENSES.md`（由 `scripts/gen-third-party.py` 生成）、`deny.toml`，CI 增加许可证检查；安装包与镜像带上许可证文件。
- 内置账号登录：会话令牌改为服务端写入的 `HttpOnly` cookie（`SameSite=Lax`，HTTPS 代理下加 `Secure`），用 cookie 的写请求要求 `x-requested-with`（防跨站请求伪造）；`x-user-token` 请求头方式保持兼容。
- 环境变量 `LEDGER_ALLOW_NO_AUTH`、`LEDGER_COOKIE_SECURE`。

- 可观测性：`GET /api/admin/metrics`（Prometheus 文本格式：请求数、耗时直方图、进行中请求、重计算排队、数据文件大小、备份状态），支持 `Authorization: Bearer`；慢请求日志（`LEDGER_SLOW_REQUEST_MS`）；`LEDGER_LOG_FORMAT=json` 结构化日志。
- 夜间规模与浸泡测试：`tests/bench/nightly.sh`、`soak.py`、`compare.py` 与 `nightly.yml`（2000 万行混合负载、内存增长与性能退步检测）。
- CI：`cargo clippy -D warnings`、macOS 单元测试、每周依赖审计（`security.yml`）、Dependabot；生产代码禁用 `unwrap()`（`clippy::unwrap_used`）。
- 数据文件结构版本（`ledger_meta`）：被更新版本写过的数据文件，旧版程序拒绝打开并提示用备份恢复；`/api/admin/status` 返回 `schemaVersion`。
- 文档：`docs/architecture.md`、`docs/formulas.md`（平台函数与 Excel 兼容）、`docs/api.md`（REST 接口参考，由源码注释与权限表生成，CI 检查是否最新）。

### 变更
- **数据权限通用化（维度）**：行级权限不再只能按「店铺」。新增「维度」（店铺 / 地区 / 部门…，随意命名）：规则按（用户，维度）设置可见取值，一个用户可以同时在几个维度上受限；数据集绑定（过滤列，维度），所以同一个用户在不同数据集上受不同规则限制；用户在数据集绑定的维度上没有规则时，该数据集对他整体可见。界面（数据权限对话框）、接口（`dimension` / `values` / `scopeDimension`，`/me` 增加 `dimensions`）同步更新，旧写法（`mode=shops`、`shops`）仍可用。**数据文件结构版本升到 2**：规则搬到新表，旧版本程序不能再打开升级后的数据文件，回退请用升级前的备份。
- **改名 Gridlake**（原名 WebExcel：与已有项目重名，且名字含微软商标 Excel）：后端目录 `ledger-server/` → `server/`，程序与 crate 名 `ledger-server` → `gridlake`，前端目录 `web/`，systemd 服务 / 安装目录 / 镜像 / 安装包 / Prometheus 指标 / 会话 cookie 一律用新名字。**保持不变**的是会写进数据里或被外部依赖的名字：`LEDGER_*` 环境变量、`ledger_` 表前缀、`/api/ledger` 接口路径、工作簿里保存的部件键（`webexcel-kpi` / `webexcel-chart` 等）、网关模式默认读取的 `webexcel-token` cookie。旧安装的迁移步骤见《部署手册》的「从 WebExcel 迁移」。
- **安全默认值（可能影响现有部署）：** 没有开启登录（`LEDGER_AUTH_MODE=off`）却监听非本机地址、或设置了 `LEDGER_CORS=*` 时，服务拒绝启动，提示改用 `local` 登录；确认已有其他保护时设置 `LEDGER_ALLOW_NO_AUTH=1`。`deploy/ledger.env.example` 默认启用 `LEDGER_AUTH_MODE=local`。
- **数据范围受限用户的接口权限改为白名单：** `auth.rs` 新增 `RESTRICTED_OK`，未登记的接口默认对受限用户关闭；测试要求每个接口明确归类。
- `LEDGER_CORS=*` 不再回显来源并允许携带凭据，改为任意来源、不带凭据。
- 登录失败计数改存数据库，重启服务不会清零。
- 前端依赖升级以修复已知高危漏洞：axios 1.20、vue 3.5.43、js-cookie 3.0.8，并用 pnpm overrides 固定 lodash / brace-expansion / minimatch / nanoid / postcss / form-data / source-map-js 的安全版本（`pnpm audit --prod` 不再有高危）。
- 前端版本号与后端统一为 0.1.0；`docker-compose.yml` 改为拉取发布镜像（`GRIDLAKE_IMAGE` 可覆盖）。

## [0.1.0]

- 开源版初始发布：亿级数据集、透视表、公式下推、SQL 工作台、仪表盘、AI 助手与 MCP、协作与权限、审批、定时推送、私有化部署。

[未发布]: https://github.com/OWNER/gridlake/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/OWNER/gridlake/releases/tag/v0.1.0
