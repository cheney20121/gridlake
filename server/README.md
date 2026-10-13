# Gridlake 服务端

Gridlake 的后端：Rust + axum + 内置 DuckDB，单进程，同时提供前端页面（设置 `LEDGER_WEB_DIR`）。项目介绍与快速开始见[根目录 README](../README.md)，部署与全部环境变量见 [deploy/部署手册.md](../deploy/部署手册.md)。

## 运行

```bash
cargo run                                   # http://127.0.0.1:8787，数据在 data/ledger.duckdb，不开启登录
LEDGER_AUTH_MODE=local cargo run            # 内置账号；首次启动的管理员密码打印在日志里
cargo run -- check                          # 离线检查数据文件（服务需停止）
cargo run -- reset-password admin --admin   # 重置内置账号密码（服务需停止，从标准输入读新密码）
```

常用环境变量：`LEDGER_ADDR`（监听地址）、`LEDGER_DB`（数据文件）、`LEDGER_WEB_DIR`（前端构建产物目录）、`LEDGER_MEMORY_LIMIT`（DuckDB 内存上限）、`LEDGER_AUTH_MODE`（off / local / gateway / proxy）、`LEDGER_AI_BASE_URL` / `LEDGER_AI_MODEL` / `LEDGER_AI_API_KEY`（AI 助手，OpenAI 兼容接口）。

## 代码结构

| 模块 | 职责 |
|---|---|
| `main.rs` / `routes/` / `config.rs` / `db.rs` / `schema.rs` | 启动与路由注册、HTTP 处理函数（`routes/` 下按功能分文件：`datasets.rs`、`workbooks.rs`、`ledger.rs` 等，共用的辅助函数在 `routes/mod.rs`）、配置、数据库连接与建表 |
| `auth.rs` / `account.rs` / `scope.rs` | 登录（四种方式）、内置账号、权限规则、操作审计、按店铺的行级权限 |
| `dataset*.rs` / `views.rs` / `calc.rs` / `usage.rs` | 数据集：导入、游标与取数、计算列、关联视图、回收站、使用统计 |
| `pivot.rs` / `pivot/` / `pivot_calc.rs` | 透视表（GROUPING SETS 一次算出所有层级）、计算值、计算项 |
| `query.rs` / `sqlq/` | 公式下推（DSUMIFS 等）、SQL 查询与参数化查询 |
| `workbook.rs` / `collab.rs` / `comment.rs` / `xref.rs` | 工作簿与版本、实时协作、评论与通知、跨工作簿引用 |
| `approval.rs` | 文件审批 |
| `ai/` / `anomaly.rs` / `mcp/` | AI 助手、异常发现、MCP 服务 |
| `connector/` / `report/` / `template/` / `recon/` | 数据连接、定时推送、模板、逐笔对账 |
| `ecommerce/`（`import` / `detail` / `balance` / `bank` / `shop` / `analysis` / `coverage` / `close` / `query`）、`routes/ecommerce.rs` | **电商资金台账场景包**（可选，`LEDGER_PACKS`）：资金流水导入与明细、店铺档案、月结锁账、余额勾稽、提现 / 银行对账、经营分析；核心代码不依赖它 |
| `aggregate.rs` / `tabular.rs` | 公式聚合下推、表格文件读取与取值转换等通用工具 |
| `admin.rs` / `web.rs` | 运维接口与备份、前端静态文件 |

## 测试

```bash
cargo test              # 单元测试
tests/e2e/run.sh        # 端到端测试：每个 test_*.py 起一个临时库的服务；tests/e2e/run.sh 30 只跑文件名含 30 的
```

压测（亿级数据）见 `tests/bench/README.md`。
