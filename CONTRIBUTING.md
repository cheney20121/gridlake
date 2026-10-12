# 参与贡献

感谢你愿意改进 Gridlake！下面是提交 Issue、改代码和发 PR 的约定。

## 先聊再做

- **Bug**：先搜一下已有 Issue，没有的话用「Bug 报告」模板新建，写清版本、部署方式、复现步骤。
- **新功能 / 较大改动**：请先开一个「功能建议」Issue 讨论方案，再动手；项目方向见 [ROADMAP.md](ROADMAP.md)。
- **安全问题**：不要公开提 Issue，见 [SECURITY.md](SECURITY.md)。

## 开发环境

需要 Rust 1.85+、Node 20.19+ 或 22.12+、pnpm、C++ 编译器（后端内置 DuckDB，首次编译约 5 ~ 15 分钟）。详细步骤见 [README](README.md#从源码运行开发)。

```bash
git config core.hooksPath .githooks      # 提交前自动做敏感内容检查（私钥、内网地址、非官方 npm 源）

cd server && cargo run            # 后端 http://127.0.0.1:8787
cd web && pnpm install && pnpm dev   # 前端 http://localhost:5173/excel/home
```

## 提交前请跑通

CI 会跑下面这些，本地先过一遍能少来回：

```bash
# 后端
cd server
cargo clippy --all-targets --locked -- -D warnings   # 警告视为错误
cargo test --locked                       # 单元测试
tests/e2e/run.sh                          # 端到端测试（tests/e2e/run.sh 30 只跑文件名含 30 的）

# 前端
cd web
pnpm lint
pnpm exec vue-tsc --noEmit -p tsconfig.app.json
pnpm test:unit
pnpm test:ui                              # 界面测试，需要本机 Chrome
pnpm i18n:check                           # 界面文案：t() 用到的中文键都要在 src/i18n/locales/en-US.json 里有英文

# 仓库根目录
scripts/check-forbidden.sh
scripts/check-version.sh                  # 后端与前端版本号一致
python3 -I scripts/gen-third-party.py --check   # 依赖的许可证；依赖变了要重新生成 THIRD_PARTY_LICENSES.md
python3 -I scripts/gen-api-docs.py --check      # 新增 / 修改接口后运行 scripts/gen-api-docs.py 重新生成 docs/api.md
```

## 代码约定

- 后端没有统一用 rustfmt 格式化（现有代码行宽较长，全量格式化会产生巨大的无关改动），所以 CI 不检查 `cargo fmt`；新代码请跟随周围的风格。

- 跟随周围代码的风格、命名和注释密度；注释和界面文字目前用中文。
- 改行为就补测试：后端逻辑放 `cargo test`，接口行为放 `tests/e2e/test_NN_*.py`，前端纯函数放 `tests/unit/`。
- **新增 HTTP 接口**：在处理函数上方写 `/// METHOD /api/路径  说明`（接口文档由它生成，缺了脚本会报错）；还必须想清楚「数据范围受限的用户」能不能调用它，并在 `server/src/auth.rs` 里登记：确认读写都经过数据范围过滤的放进 `RESTRICTED_OK`，会绕过过滤的放进 `FULL_ONLY`。受限用户**默认调不了**未登记的接口，`routes_are_all_classified` 测试也会在漏登记时失败。
- **界面文案要能翻译**：新增的界面文字写成 `t('中文原文')`（模板里 `$t('…')`），带变量用占位符 `t('共 {0} 行', [n])`，再在 `web/src/i18n/locales/en-US.json` 里补英文，`pnpm i18n:check` 会提醒漏掉的。中文原文就是键，所以改了原文要同步改 JSON。**不要翻译会当数据用的字符串**：数据集 / 列名、公式里的字符串、按名字查找或比较用的标签；这些翻译后英文界面会查不到数据。
- 涉及 SQL 拼接一律走 `schema::ident` / `schema::literal` 或参数绑定，不要直接拼用户输入。
- 不要提交：数据库文件、`secret.key`、`.env.local`、真实客户数据、公司内部域名或账号。示例数据请用合成数据。

## 提交与 PR

- 一个 PR 只做一件事，尽量小。标题说明做了什么，正文说明为什么、怎么验证的。
- 提交说明用中文或英文均可，一句话讲清改动，不要混入无关的格式化。
- PR 会按模板自查；CI 全绿后由维护者审阅。有用户可见的变化请在 [CHANGELOG.md](CHANGELOG.md) 的「未发布」下加一行。

## 许可证

提交即表示你同意你的贡献以 [Apache-2.0](LICENSE) 许可证发布。
