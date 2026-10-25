# 文档

| 文档 | 内容 |
|---|---|
| [用户手册.md](用户手册.md) | **操作手册**：按「要做的事」讲怎么用——导入数据、浏览大数据表、汇总与分类汇总、整理与清洗、校验、透视与图表、公式、SQL、仪表盘、AI 助手、自动化、协作与权限 |
| [应用场景.md](应用场景.md) | **业务场景**：解决哪些用户需求；电商经营分析、财务对账、门店填报、库存、数据质量、自助分析、对外交付等场景的具体做法；不适合的场景 |
| [项目亮点.md](项目亮点.md) | **项目介绍与推广**：十个亮点、性能数据、与同类工具对比、技术亮点、可直接使用的介绍文案与演示脚本（英文版 [highlights.en.md](highlights.en.md)） |
| [architecture.md](architecture.md) | 架构、数据目录、关键设计 |
| [formulas.md](formulas.md) | 平台函数（D / T 系列、单元格里的 SQL）、Excel 表格引用转换、xlsx 兼容 |
| [excel-functions.md](excel-functions.md) | Excel 函数兼容清单：逐个函数列出支持情况、不支持的原因与替代（由表格内核的函数目录生成，`pnpm formula:catalog`） |
| [api.md](api.md) | REST 接口参考（由源码生成，`scripts/gen-api-docs.py`） |
| [openapi.json](openapi.json) | 同一份接口的 OpenAPI 3.0 描述（路径、权限、路径 / 查询参数、响应信封），可导入 Postman / Swagger UI / 代码生成器；请求体字段由 Rust 的 serde 结构体推出（`scripts/rust_schema.py`） |
| [../deploy/部署手册.md](../deploy/部署手册.md) | 安装、配置项、登录与权限、备份恢复、监控、升级 |
| [../server/tests/bench/README.md](../server/tests/bench/README.md) | 规模压测与夜间浸泡测试 |
| [../ROADMAP.md](../ROADMAP.md) | 后续计划 |
