#!/usr/bin/env bash
# 检查后端（Cargo.toml）与前端（package.json）的版本号一致；传入版本号参数时还要与它一致（发布时传标签）
#
#   scripts/check-version.sh            只检查两处一致
#   scripts/check-version.sh 0.2.0      还要求等于 0.2.0
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

server="$(grep -m1 '^version' ledger-server/Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
web="$(grep -m1 '"version"' demo-web-vue3/package.json | sed -E 's/.*"version": *"([^"]*)".*/\1/')"

echo "ledger-server/Cargo.toml     $server"
echo "demo-web-vue3/package.json   $web"

fail=0
if [ "$server" != "$web" ]; then
  echo "错误：后端与前端版本号不一致" >&2
  fail=1
fi
if [ -n "${1:-}" ] && [ "$server" != "$1" ]; then
  echo "错误：版本号 $server 与期望的 $1 不一致（发版前先改 Cargo.toml、Cargo.lock 和 package.json）" >&2
  fail=1
fi
exit $fail
