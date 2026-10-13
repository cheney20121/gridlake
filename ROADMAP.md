# Gridlake 后续开发路线（ROADMAP）

> 本文是对当前代码库的一次盘点：现有功能、作为开源项目的缺口，以及按优先级排列的后续需求。
> 依据是对仓库的静态阅读（README、部署手册、后端 / 前端目录与关键模块、CI 配置、测试目录），**未运行编译与测试**；测试数量为静态统计，不代表全部通过。
> 工作量：S ≤ 2 天，M ≈ 1 周，L > 2 周。

## 一、现有功能

产品定位是“Excel 式界面 + 服务端 DuckDB”，用来分析亿级数据。后端约 3.2 万行 Rust（axum + DuckDB），前端约 4.5 万行 Vue 3 + Univer。

| 模块 | 已有能力 |
|---|---|
| 大数据集 | 导入 CSV / Excel / Parquet / JSON；连接 ClickHouse、MySQL / PG、HTTP 接口（分页、增量、加密存密钥）；筛选、排序、计算列、关联视图、回收站 |
| 透视表 | 服务端 GROUPING SETS 计算；计算值、计算项、占比 / 同比 / 环比、近似分位数；下钻核对；可转为数据表 |
| 公式下推 | `SUMIFS(表[列],…)` 在服务端算；`DSUMIFS` 等平台函数；导出 xlsx 时转回原生公式 |
| SQL 工作台 | 只读 SQL，用 DuckDB 解析器做白名单检查；参数化查询；另存为数据集；定时重算 |
| 仪表盘 | 切片器联动、指标卡、图表、演示模式 |
| AI | 接 OpenAI 兼容模型；方案卡确认后才写入，可撤销；异常发现；MCP 只读服务，令牌可限定数据集 |
| 协作与治理 | 实时协作、评论 / @、四种登录方式、操作审计、行级权限、文件审批 / 月结审批、通知、版本历史 |
| 其他 | 填报回写（可撤销）、xlsx 导入导出、模板、定时推送到企微 / 钉钉 / 飞书 / 邮件、备份 / 恢复、Docker / systemd 部署 |
| 测试 | 后端单元约 124 个、e2e 30 个文件，前端单元约 188 个、界面测试约 60 个，CI 已覆盖 |

## 二、作为开源项目的评估

**做得好的地方：**
- 许可证（Apache-2.0）和 CI 俱全。
- 有敏感内容检查（脚本、提交前钩子、CI）。
- 性能数据如实列出，包括偏慢的操作。
- “现状与局限”一节写得诚实。
- 安全设计有章法：SQL 白名单、密钥 AES-GCM 加密、MCP 令牌只存摘要、AI 写入需确认。

**主要缺口：**
1. 社区文件一个都没有（CONTRIBUTING、SECURITY、CODE_OF_CONDUCT、CHANGELOG、Issue / PR 模板），也没有 `docs/` 目录。
2. 没有 git remote，只有 4 个提交，没有 tag 和发布物，没有截图或在线 Demo。
3. 安装门槛高：README 要求本地 `docker build`，源码编译 DuckDB 要 5 ~ 15 分钟。
4. 定位偏窄：约 6000 行代码是电商资金台账专用逻辑（bank / shop / balance / close / 抖店连接器等），和通用核心混在一起。
5. 只有中文：界面和后端报错都是中文，没有英文文档。

## 三、需求清单

### P0：公开发布前必须完成（已完成，2026-10-12）

| # | 需求 | 依据 | 量 | 状态 |
|---|---|---|---|---|
| 1 | 补齐社区文件：CONTRIBUTING、SECURITY（漏洞披露渠道）、CODE_OF_CONDUCT、CHANGELOG、Issue / PR 模板 | 仓库根目录缺失 | S | ✅ 已补齐 |
| 2 | 发布物：GitHub Release 提供 Linux x86 / arm 和 macOS 二进制，镜像推到 GHCR；`docker-compose.yml` 改为拉取镜像；统一版本号 | 后端 0.1.0、前端 0.0.0 | M | ✅ 已实现，待首次打标签验证 |
| 3 | README 加截图或 GIF、在线 Demo（带示例数据集一键体验）、英文简介 | README 无任何图 | S | ✅ 截图、`seed-demo.py`、英文 README；在线 Demo 需自行部署 |
| 4 | **权限从黑名单改白名单。** `auth.rs` 的 `FULL_ONLY` 是“受限用户不能调用的接口”清单，新增路由默认对受限用户开放，容易漏。改成默认拒绝，或加测试断言每条路由都已分类 | `server/src/auth.rs` 的 `FULL_ONLY` | M | ✅ `RESTRICTED_OK` 白名单 + 分类测试 |
| 5 | **安全默认值：** `AUTH_MODE=off` 同时监听 `0.0.0.0` 时启动即警告或拒绝；`LEDGER_CORS=*` 用了 `very_permissive`，加警告 | `config.rs`、`main.rs` | S | ✅ 拒绝启动 + 警告，`LEDGER_ALLOW_NO_AUTH` |
| 6 | **登录令牌放在 JS 可读的 cookie 里**（用于 WebSocket 和下载链接）。改为服务端下发 HttpOnly cookie，并补 CSRF 防护；登录失败锁定目前存在内存里，重启清零，可持久化 | `web/src/utils/auth.ts`、`account.rs` | M | ✅ HttpOnly cookie + `x-requested-with` + 失败计数入库 |
| 7 | 开源合规：用 cargo-deny / license-checker 生成第三方许可证清单和 NOTICE，查“Gridlake”重名与商标，复查 `templates/*.json` 里的业务示例数据是否脱敏 | 仓库内无 NOTICE | S | ✅ NOTICE / 许可证清单 / CI 检查；重名见下 |

**P0 落地后的遗留事项（需要人工决定 / 操作）：**

- 仓库发布到 GitHub 后运行 `scripts/set-repo.sh 用户名/仓库名`，替换文档和镜像地址里的 `OWNER` 占位符；再打 `v0.1.0` 标签验证发布流水线（Linux arm64 / macOS 构建、GHCR 推送尚未实跑）。
- **项目名：** 原名 WebExcel 与已有的同名项目重名，且包含微软的商标 Excel，已改名为 Gridlake（npm、crates.io 上无同名包，GitHub 上无同名仓库）。正式推广前仍建议到 USPTO / WIPO / 国内商标局核查 “Gridlake”——检索不等于商标查询。
- 示例模板（`server/templates/*.json`）已核对：只含列定义和说明文字，没有真实公司 / 客户 / 联系方式。
- 在线 Demo 需要自己部署一个实例（`seed-demo.py` 可导入数据），并建议只读或定时重置。

### P1：让社区能用起来、能参与

| # | 需求 | 说明 | 量 |
|---|---|---|---|
| 8 | ✅ **通用化：** 电商台账已拆成可选“场景包”（`server/src/ecommerce/`、`web/src/packs/ecommerce/`，`LEDGER_PACKS`），核心只保留数据集 / 透视 / 公式 / SQL / 仪表盘；库里没有台账数据时默认不启用 | 降低陌生用户的理解成本 | L |
| 9 | 国际化：**前端已完成大半**——自写的 `t('中文原文')`（`web/src/i18n/`，缺译文回退中文）、语言切换器、约 2000 条英文，Element Plus / Univer 界面同步切换，`pnpm i18n:check` 进 CI；后端错误信息也已支持英文（`server/src/msg_en.json`，约 700 条模板，请求带 `x-lang` / cookie `gridlake-lang` 时翻译）；**未完成**：约 670 行前端脚本里的运行时提示和标签（逐个确认过的才会翻译：和服务端对暗号的名字、会进列别名的名称、搜索关键词等保持中文）、电商台账场景（有意保持中文）、AI 与 MCP 的工具说明、定时推送的消息正文 | 目前后端报错直接写中文字符串 | L |
| 10 | 建 `docs/`：架构说明、平台函数手册（`DSUMIFS` 等）、Excel 公式兼容矩阵、REST API 参考（OpenAPI）、配置项参考；部署手册文件名改英文 | 现在配置只在 `deploy/` 手册里 | M |
| 11 | CI 增强：`cargo fmt` / `clippy -D warnings`、`cargo audit`、`pnpm audit`、Dependabot / Renovate、macOS / Linux 矩阵、覆盖率徽章 | 当前 CI 只有测试和构建 | S |
| 12 | 代码拆分：`routes.rs`（2030 行）已按功能拆成 `routes/` 下 13 个文件；`ai/tools.rs`（1836）、`views/excel/index.vue`（934）尚未拆；目录 `demo-web-vue3` 已改名为 `web` | 降低贡献门槛 | M |
| 13 | 评估 452 处 `unwrap/expect`（含测试代码）：生产路径换成错误传播，或加 `#![deny(clippy::unwrap_used)]` | 只统计了总数，未区分生产和测试 | M |
| 14 | Linux 长时间运行验证（soak test）：把 `tests/bench` 的亿级压测做成 nightly CI，建立性能回归基线 | README 自述“没在 Linux 生产长期运行过” | M |
| 15 | 数据文件升级 / 回滚策略：schema 版本号、迁移说明、备份校验、一键恢复界面 | 升级章节只有手册里的几行 | M |
| 16 | 可观测性：Prometheus `/metrics`、结构化日志、慢查询日志、重计算队列长度 | 现有健康检查和审计，缺运行指标 | S |

### P2：功能增强

| # | 需求 | 说明 | 量 |
|---|---|---|---|
| 17 | 认证扩展：原生 OIDC / LDAP / SAML、2FA、~~个人访问令牌（PAT）供脚本调用~~（✅ 已完成，见 `server/src/pat.rs`） | 现在 OIDC 要靠反向代理 | L |
| 18 | 权限通用化：**部分完成**——每个数据集可自选过滤列、界面文案已改为“按某一列的取值”；**未做**：规则的值列表仍叫 `shops`（API 命名）、同一个用户在不同数据集用不同的规则、列级权限、用户组 / 角色 | `scope.rs` 里还留着“店铺”概念 | L |
| 19 | 更多连接器：S3 / 对象存储、Snowflake / BigQuery、MongoDB、Google Sheets，以及增量同步（CDC） | 现有 ClickHouse / MySQL / PG / HTTP / 文件夹 | L |
| 20 | 更多下推函数：`DAVERAGEIFS`、`DMAXIFS`、`XLOOKUP` 等；公式诊断覆盖更多不支持的写法 | 与 #10 的兼容矩阵一起做 | M |
| 21 | AI：建评测集和回归基准、多模型 / 按用户配置、提示注入防护、成本与限流、本地模型适配文档 | README 承认“没有系统评测” | L |
| 22 | 分享与嵌入：只读分享链接、iframe / SDK 嵌入仪表盘、导出 PDF / 打印 | 目前有演示模式，没有对外分享 | M |
| 23 | 备份增强：备份到 S3、校验、定期恢复演练 | 目前只有本地目录加保留份数 | S |
| 24 | ✅ SSRF：`netguard.rs` 统一检查网址导入与推送 Webhook（连接时过滤、重定向逐跳检查、IPv4 映射 / NAT64 / 云元数据地址），`LEDGER_URL_BLOCK_PRIVATE` 可选拒绝内网。数据连接只有管理员能建，仍可访问内网（设计如此） | — | S |
| 25 | 多工作空间 / 多租户隔离：目前是单实例单库 | 企业用户常见诉求，需先评估成本 | L |

### P3：长期探索

- 十亿行规模验证，或分区 / 外部 Parquet 表；
- 多节点读副本、高可用；
- 插件 / UDF 扩展机制、事件 Webhook；
- 移动端只读查看、桌面版封装。

## 四、建议路线

1. **第 1 个月（发布准备）：** P0 全部，加 #11、#10 的基础版。完成后再公开仓库。
2. **第 2 ~ 3 个月（降低门槛）：** #8 通用化、#9 英文化、#12 目录和代码拆分，用 #14 的基线保证不退化。
3. **之后** 按社区反馈在 P2 里选，建议先做 #17 认证扩展和 #19 连接器。
