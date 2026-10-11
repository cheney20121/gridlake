# 更新日志

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [未发布]

### 新增
- 社区文件：CONTRIBUTING、SECURITY、CODE_OF_CONDUCT、Issue / PR 模板。
- 发布流水线：推送 `v*` 标签自动构建 Linux x86_64 / aarch64、macOS arm64 安装包并发布 GitHub Release，同时推送多架构镜像到 GHCR；`scripts/check-version.sh` 校验版本一致；`scripts/set-repo.sh` 替换文档里的仓库占位符。
- README：截图、一分钟体验、英文简介（`README.en.md`）；`scripts/seed-demo.py` 一键导入示例订单数据。
- 开源合规：`NOTICE`、`THIRD_PARTY_LICENSES.md`（由 `scripts/gen-third-party.py` 生成）、`deny.toml`，CI 增加许可证检查；安装包与镜像带上许可证文件。
- 内置账号登录：会话令牌改为服务端写入的 `HttpOnly` cookie（`SameSite=Lax`，HTTPS 代理下加 `Secure`），用 cookie 的写请求要求 `x-requested-with`（防跨站请求伪造）；`x-user-token` 请求头方式保持兼容。
- 环境变量 `LEDGER_ALLOW_NO_AUTH`、`LEDGER_COOKIE_SECURE`。

### 变更
- **安全默认值（可能影响现有部署）：** 没有开启登录（`LEDGER_AUTH_MODE=off`）却监听非本机地址、或设置了 `LEDGER_CORS=*` 时，服务拒绝启动，提示改用 `local` 登录；确认已有其他保护时设置 `LEDGER_ALLOW_NO_AUTH=1`。`deploy/ledger.env.example` 默认启用 `LEDGER_AUTH_MODE=local`。
- **数据范围受限用户的接口权限改为白名单：** `auth.rs` 新增 `RESTRICTED_OK`，未登记的接口默认对受限用户关闭；测试要求每个接口明确归类。
- `LEDGER_CORS=*` 不再回显来源并允许携带凭据，改为任意来源、不带凭据。
- 登录失败计数改存数据库，重启服务不会清零。
- 前端版本号与后端统一为 0.1.0；`docker-compose.yml` 改为拉取发布镜像（`WEBEXCEL_IMAGE` 可覆盖）。

## [0.1.0]

- 开源版初始发布：亿级数据集、透视表、公式下推、SQL 工作台、仪表盘、AI 助手与 MCP、协作与权限、审批、定时推送、私有化部署。

[未发布]: https://github.com/OWNER/webexcel/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/OWNER/webexcel/releases/tag/v0.1.0
