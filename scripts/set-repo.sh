#!/usr/bin/env bash
# 仓库发布到 GitHub 后，把文档与配置里的占位符 OWNER/REPO 换成真实的「用户名/仓库名」：
#
#   scripts/set-repo.sh myname/gridlake
#
# 涉及：CHANGELOG 的比较链接、Issue 模板的安全报告链接、docker-compose 的镜像地址、README 的镜像与安装包地址
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

repo="${1:-}"
[[ "$repo" =~ ^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$ ]] || { echo "用法：scripts/set-repo.sh 用户名/仓库名" >&2; exit 1; }
owner="${repo%%/*}"
name="${repo##*/}"
# GHCR 镜像地址必须是小写
image="ghcr.io/$(printf '%s' "$repo" | tr '[:upper:]' '[:lower:]')"

files="$(git grep -l -E 'OWNER/REPO|OWNER/gridlake|ghcr\.io/OWNER' -- . ':!scripts/set-repo.sh' || true)"
[ -n "$files" ] || { echo "没有找到占位符（可能已经替换过）"; exit 0; }

for f in $files; do
  # 兼容 macOS 与 GNU sed：不用 -i，写临时文件再覆盖
  sed -e "s#ghcr\.io/OWNER/gridlake#$image#g" -e "s#OWNER/REPO#$repo#g" -e "s#OWNER/gridlake#$owner/$name#g" "$f" > "$f.tmp"
  cat "$f.tmp" > "$f" && rm -f "$f.tmp"
  echo "已更新 $f"
done
