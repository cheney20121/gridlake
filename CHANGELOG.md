# 更新日志

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [未发布]

### 新增
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
