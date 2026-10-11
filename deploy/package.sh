#!/usr/bin/env bash
# 打私有化安装包：前端构建产物 + 表格服务二进制 + 配置模板 + systemd 服务 + 部署手册
#
#   ./deploy/package.sh            # 在目标平台（如 Linux x86_64 构建机）上执行，产出 dist/webexcel-<版本>-<系统>-<架构>.tar.gz
#   SKIP_WEB=1 ./deploy/package.sh # 只重新打后端
#
# 依赖：Node 20+ 与 pnpm（前端）、Rust 1.85+ 与 C++ 编译器（后端内置 DuckDB，首次编译约 5 分钟）
# 没有 Linux 构建机时用 Docker：docker build -f deploy/Dockerfile -t webexcel .
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WEB="$ROOT/demo-web-vue3"
SERVER="$ROOT/ledger-server"
VERSION="$(grep -m1 '^version' "$SERVER/Cargo.toml" | sed -E 's/.*"(.*)".*/\1/')"
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
NAME="webexcel-$VERSION-$OS-$ARCH"
OUT="$ROOT/dist/$NAME"

echo "==> 打包 $NAME"
rm -rf "$OUT" && mkdir -p "$OUT/bin" "$OUT/web"

if [[ "${SKIP_WEB:-}" != "1" ]]; then
  echo "==> 构建前端"
  (cd "$WEB" && pnpm install --frozen-lockfile && node_modules/.bin/vite build --outDir "$OUT/web" --emptyOutDir)
  # 预压缩：服务端直接返回 .gz，大体积脚本不必每次压缩
  find "$OUT/web" -type f \( -name '*.js' -o -name '*.css' -o -name '*.html' -o -name '*.svg' -o -name '*.json' -o -name '*.wasm' \) -size +1k \
    ! -path '*/config/*' -exec gzip -k -9 -f {} \;
fi

echo "==> 构建表格服务"
(cd "$SERVER" && cargo build --release --locked)
cp "$SERVER/target/release/ledger-server" "$OUT/bin/"
strip "$OUT/bin/ledger-server" 2>/dev/null || true

cp "$ROOT/deploy/ledger.env.example" "$ROOT/deploy/webexcel.service" "$ROOT/deploy/install.sh" "$ROOT/deploy/部署手册.md" "$OUT/"
# 验收用的端到端测试（用临时数据库 + 模拟网关，不碰生产数据）：安装后 cd /opt/webexcel && tests/e2e/run.sh
mkdir -p "$OUT/tests" && cp -r "$SERVER/tests/e2e" "$OUT/tests/" && rm -rf "$OUT/tests/e2e/__pycache__"
chmod +x "$OUT/install.sh" "$OUT/bin/ledger-server" "$OUT/tests/e2e/run.sh"
echo "$VERSION" > "$OUT/VERSION"

(cd "$ROOT/dist" && tar -czf "$NAME.tar.gz" "$NAME" && shasum -a 256 "$NAME.tar.gz" > "$NAME.tar.gz.sha256")
echo "==> 完成：dist/$NAME.tar.gz（$(du -h "$ROOT/dist/$NAME.tar.gz" | cut -f1)）"
cat "$ROOT/dist/$NAME.tar.gz.sha256"
