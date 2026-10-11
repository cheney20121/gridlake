# WebExcel

**开源、可私有部署的亿级数据表格**：像 Excel 一样用公式和透视表分析上亿行数据。表格界面基于 [Univer](https://github.com/dream-num/univer)，数据放在服务端的 [DuckDB](https://github.com/duckdb/duckdb) 里，浏览器只取看到的那一屏。

> An open-source, self-hosted spreadsheet for 100M-row datasets — Excel-style formulas and pivot tables on top of DuckDB. The UI is Chinese-only for now.

## 为什么

- Excel 最多 104 万行，数据一多就打不开；
- BI 工具做固定看板很好，但没有 Excel 那样随手写公式、拖透视、自由排版的灵活；
- 数据库式表格（Airtable / NocoDB / Teable 一类）是「每列一个类型、一行一条记录」，不是 Excel 式的自由表格。

WebExcel 的做法是：**界面是 Excel 式的表格，大数据集在服务端 DuckDB 里计算**。普通工作表、公式、图表照常在浏览器里；引用大数据集的透视表、公式、数据表、图表都下推到服务端算。

## 性能（实测）

在 1 亿行 × 8 列的合成订单数据上（MacBook 8 核 16 GB，发布版服务，通过 HTTP 接口测得；浏览器里看到的耗时会稍长）：

| 操作 | 耗时 |
|---|---|
| 打开整张表（不建任何中间表） | 52 毫秒 |
| 按店铺筛选（命中 33 万行） | 约 0.1 秒 |
| 透视：店铺 × 月份 | 1.8 秒 |
| SQL 分组汇总 | 0.5 秒 |
| 公式 `DSUMIFS` 单个（两个条件） | 0.17 秒 |
| 导出 33 万行 CSV | 1.8 秒 |

偏慢的操作也如实列出：冷启动全表排序 11.6 ~ 18 秒（之后同条件 3 毫秒）、1 亿唯一值的精确去重计数 17 ~ 44 秒（可改用近似去重）。复现方法见 `ledger-server/tests/bench/README.md`。

## 功能

- **大数据集**：导入 CSV / Excel / Parquet / JSON，连接 ClickHouse、MySQL / PostgreSQL、HTTP 接口；筛选、排序、搜索、计算列、关联视图、回收站
- **透视表**：服务端计算，写成普通工作表；计算值（毛利率等）、计算项、首个 / 末个、占比 / 同比 / 环比、近似分位数；每个格子可下钻到明细并自动核对；可「转为数据表」再按值筛选
- **公式下推**：`=SUMIFS(订单表[金额], 订单表[店铺], B2)` 这类写法在服务端算；`DSUMIFS` / `DCOUNTIFS` 等平台函数；导出 xlsx 时转回 Excel 原生公式
- **数据表嵌入工作表**：浮动表格或直接写成单元格，随数据集与切片器刷新
- **SQL 工作台**：只读 SQL、参数化查询（按类型安全代入）、另存为数据集、定时重算
- **仪表盘**：切片器（值 / 日期，跨数据集联动）、指标卡、图表联动、演示模式
- **AI 助手**（接你自己的 OpenAI 兼容模型）：给出方案卡，确认后才写入、可撤销；异常发现；外部 AI 通过 MCP 只读查询
- **协作与治理**：实时协作、评论与 @ 提醒；登录（内置账号 / 网关 / 反向代理）、操作审计、按店铺的行级权限、文件审批与月结审批、应用内通知
- **其他**：xlsx 导入导出（条件格式、表格、行列分组、命名区域）、模板（可参数化）、定时推送到企业微信 / 钉钉 / 飞书 / 邮件、版本历史

内置了一组电商财务场景的模板（资金台账、多平台利润表、应收应付对账、库存进销存、银行余额调节表），这是项目最早的使用场景。

## 快速开始

### Docker

```bash
docker build -f deploy/Dockerfile -t webexcel:0.1.0 .
mkdir -p data && sudo chown -R 10001:10001 data
docker compose -f deploy/docker-compose.yml up -d
docker logs webexcel 2>&1 | grep 初始密码      # 首次启动随机生成的管理员密码
```

浏览器打开 `http://localhost:8787/excel`，用 `admin` 和上面的密码登录，然后在首页「自动化 → 账号管理」里给同事建账号。

### 从源码运行（开发）

需要 Rust 1.85+、Node 20+ 与 pnpm、C++ 编译器（后端内置 DuckDB，首次编译约 5 ~ 15 分钟）。

```bash
# 后端（默认 http://127.0.0.1:8787，不开启登录；加 LEDGER_AUTH_MODE=local 使用内置账号）
cd ledger-server && cargo run

# 前端（另开一个终端，http://localhost:5173/excel/home）
cd demo-web-vue3 && pnpm install && pnpm dev
```

macOS 上如果编译 DuckDB 报 `'memory' file not found`，加上：`export CXXFLAGS="-isystem $(xcrun --show-sdk-path)/usr/include/c++/v1"`。

### 私有化部署

安装包、systemd、备份恢复、HTTPS、登录方式与权限见 [deploy/部署手册.md](deploy/部署手册.md)。

## 登录方式

| `LEDGER_AUTH_MODE` | 说明 |
|---|---|
| `off`（默认） | 不登录，任何能访问页面的人都可以使用全部功能。只适合本机或内网试用 |
| `local` | 内置账号：管理员在页面里建账号；密码 PBKDF2-SHA256 保存，失败次数限制，登录记入审计 |
| `gateway` | 对接已有的统一登录：浏览器带的令牌由服务端转给网关校验 |
| `proxy` | 由反向代理（nginx `auth_request`、OAuth2 Proxy 等）传入身份，可接 OIDC / LDAP |

## 目录

| 目录 | 内容 |
|---|---|
| `ledger-server/` | 后端：Rust + axum + DuckDB（数据集、透视、公式下推、SQL、AI、协作、权限、审批） |
| `demo-web-vue3/` | 前端：Vue 3 + Univer + Element Plus |
| `deploy/` | Dockerfile、docker-compose、安装脚本、部署手册 |

## 测试

```bash
cd ledger-server && cargo test                       # 后端单元测试
cd ledger-server && tests/e2e/run.sh                 # 后端端到端测试（每个文件起一个临时库的服务）
cd demo-web-vue3 && pnpm test:unit                   # 前端单元测试
cd demo-web-vue3 && pnpm test:ui                     # 界面测试（需要本机 Chrome）
scripts/check-forbidden.sh                           # 敏感内容检查：私钥、内网地址、非官方 npm 源
git config core.hooksPath .githooks                  # 提交前自动做上面的检查
```

## 现状与局限

- 单机 DuckDB，没有分布式；「十亿行」没有测过
- 界面只有中文
- AI 助手的效果取决于接入的模型，还没有系统评测
- 还没有在 Linux 生产环境长期运行过；欢迎反馈问题

## 许可证

[Apache-2.0](LICENSE)。依赖的 Univer（Apache-2.0）、DuckDB（MIT）等各自的许可证见对应项目。
