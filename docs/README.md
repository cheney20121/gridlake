# 文档

| 文档 | 内容 |
|---|---|
| [architecture.md](architecture.md) | 架构、数据目录、关键设计 |
| [formulas.md](formulas.md) | 平台函数（D / T 系列、单元格里的 SQL）、Excel 表格引用转换、xlsx 兼容 |
| [excel-functions.md](excel-functions.md) | Excel 函数兼容清单：逐个函数列出支持情况、不支持的原因与替代（由表格内核的函数目录生成，`pnpm formula:catalog`） |
| [api.md](api.md) | REST 接口参考（由源码生成，`scripts/gen-api-docs.py`） |
| [openapi.json](openapi.json) | 同一份接口的 OpenAPI 3.0 描述（路径、权限、路径 / 查询参数、响应信封），可导入 Postman / Swagger UI / 代码生成器；请求体字段由 Rust 的 serde 结构体推出（`scripts/rust_schema.py`） |
| [../deploy/部署手册.md](../deploy/部署手册.md) | 安装、配置项、登录与权限、备份恢复、监控、升级 |
| [../server/tests/bench/README.md](../server/tests/bench/README.md) | 规模压测与夜间浸泡测试 |
| [../ROADMAP.md](../ROADMAP.md) | 后续计划 |
