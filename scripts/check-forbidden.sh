#!/usr/bin/env bash
# 提交 / 发布前的敏感内容检查：私钥、内网地址、非官方 npm 源，以及维护者自己配置的敏感词。
#
#   scripts/check-forbidden.sh            检查仓库里所有受版本控制的文件和全部提交说明
#   scripts/check-forbidden.sh --staged   只检查暂存区（pre-commit 钩子用）
#
# 敏感词（公司名、内部域名等）不能写进仓库本身，按以下顺序读取，每行一个扩展正则，不区分大小写，# 开头为注释：
#   1. 环境变量 FORBIDDEN_PATTERNS（CI 里来自仓库 Secret）
#   2. 文件 $FORBIDDEN_PATTERNS_FILE，默认仓库根目录的 .forbidden-patterns（已在 .gitignore 里）
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

mode=all
[ "${1:-}" = "--staged" ] && mode=staged

# 通用规则（可以公开）：私钥、内网 IP 地址；npm 源在后面单独检查
generic='-----BEGIN [A-Z ]*PRIVATE KEY-----
https?://(10|192\.168|172\.(1[6-9]|2[0-9]|3[01]))\.[0-9]+\.[0-9]+\.[0-9]+'

private="${FORBIDDEN_PATTERNS:-}"
file="${FORBIDDEN_PATTERNS_FILE:-.forbidden-patterns}"
if [ -z "$private" ] && [ -f "$file" ]; then
  private="$(cat "$file")"
fi
if [ -z "$private" ]; then
  echo "提示：没有配置敏感词（FORBIDDEN_PATTERNS 或 $file），只做通用检查" >&2
fi

patterns="$(printf '%s\n%s\n' "$generic" "$private" | grep -v '^[[:space:]]*#' | grep -v '^[[:space:]]*$' || true)"
pfile="$(mktemp)"
trap 'rm -f "$pfile"' EXIT
printf '%s\n' "$patterns" > "$pfile"

# 本脚本自身含有规则文本，不检查
exclude=(':!scripts/check-forbidden.sh')

found=0
if [ "$mode" = staged ]; then
  hits="$(git grep --cached -n -I -i -E -f "$pfile" -- . "${exclude[@]}" || true)"
else
  hits="$(git grep -n -I -i -E -f "$pfile" -- . "${exclude[@]}" || true)"
fi
if [ -n "$hits" ]; then
  echo "发现敏感内容：" >&2
  # 只显示位置，不显示内容：公开仓库的 CI 日志也是公开的
  printf '%s\n' "$hits" | cut -d: -f1-2 >&2
  found=1
fi

# 锁文件只允许官方源
lock_hits="$( (git ls-files '*pnpm-lock.yaml' '*package-lock.json' '*.npmrc' | while read -r f; do
  grep -n -E '(resolved|tarball|registry)"?[[:space:]]*[:=][[:space:]]*"?https?://' "$f" | grep -v -E 'registry\.npmjs\.org|github\.com|codeload\.github\.com' | sed "s|^|$f:|"
done) || true)"
if [ -n "$lock_hits" ]; then
  echo "锁文件 / .npmrc 里有非官方 npm 源：" >&2
  printf '%s\n' "$lock_hits" | head -20 | cut -d: -f1-2 >&2
  found=1
fi

# 提交说明（全量模式）
if [ "$mode" = all ] && git rev-parse -q --verify HEAD >/dev/null; then
  msg_hits="$(git log --format=%h | while read -r h; do
    git log -1 --format=%B "$h" | grep -q -i -E -f "$pfile" && echo "$h"
  done || true)"
  if [ -n "$msg_hits" ]; then
    echo "这些提交的说明里有敏感内容：" >&2
    printf '%s\n' "$msg_hits" >&2
    found=1
  fi
fi

if [ "$found" = 1 ]; then
  exit 1
fi
echo "敏感内容检查通过"
