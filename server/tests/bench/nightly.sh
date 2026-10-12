#!/usr/bin/env bash
# 夜间 / 手动的规模与浸泡测试：造数据 → 导入 → 放大到目标行数 → 混合负载 N 分钟（soak.py）→ 与上次结果对比（compare.py）
#
#   tests/bench/nightly.sh                      # 默认：基础 200 万行放大到 2000 万行，混合负载 15 分钟
#   BASE_ROWS=1000000 TARGET_ROWS=5000000 SOAK_MINUTES=2 tests/bench/nightly.sh     # 本机快速试跑
#
# 环境变量：BASE_ROWS、TARGET_ROWS、SOAK_MINUTES、WORKERS（并发，默认 4）、WORKDIR（临时目录，默认 mktemp）、
#           PREV（上次结果 json，用来对比；没有就只记录）、RESULT（本次结果输出路径，默认 soak-result.json）、
#           BIN（服务程序，默认 target/release/gridlake；本机试跑可指向 debug 版）、KEEP=1（不删临时目录）
# 需要：已 cargo build --release 并 cargo build --release --example inflate；python3；磁盘约 2 GB / 千万行
set -euo pipefail
cd "$(dirname "$0")/../.."

BASE_ROWS="${BASE_ROWS:-2000000}"
TARGET_ROWS="${TARGET_ROWS:-20000000}"
SOAK_MINUTES="${SOAK_MINUTES:-15}"
WORKERS="${WORKERS:-4}"
RESULT="${RESULT:-$PWD/soak-result.json}"
WORKDIR="${WORKDIR:-$(mktemp -d)}"
BIN="${BIN:-target/release/gridlake}"
INFLATE="${INFLATE:-target/release/examples/inflate}"
[ -x "$BIN" ] || { echo "找不到服务程序 $BIN：先执行 cargo build --release"; exit 2; }
if [ "$TARGET_ROWS" -gt "$BASE_ROWS" ] && [ ! -x "$INFLATE" ]; then
  echo "找不到 $INFLATE：先执行 cargo build --release --example inflate"; exit 2
fi

mkdir -p "$WORKDIR/data"
export LEDGER_ADDR=127.0.0.1:18950 LEDGER_DB="$WORKDIR/data/ledger.duckdb" LEDGER_BACKUP_AT=off LEDGER_E2E_BASE=http://127.0.0.1:18950/api LEDGER_E2E_TIMEOUT=900
SRV=
cleanup() {
  if [ -n "$SRV" ]; then kill "$SRV" 2>/dev/null || true; wait "$SRV" 2>/dev/null || true; fi
  [ -z "${KEEP:-}" ] && rm -rf "$WORKDIR"
  return 0
}
trap cleanup EXIT
start() {
  "$BIN" > "$WORKDIR/server.log" 2>&1 &
  SRV=$!
  for _ in $(seq 1 100); do curl -s -o /dev/null "http://127.0.0.1:18950/api/health" && return 0; sleep 0.3; done
  echo "服务没有启动"; tail -20 "$WORKDIR/server.log"; exit 1
}

echo "==> 生成基础数据 $BASE_ROWS 行"
ROOT="$PWD"
(cd "$WORKDIR" && python3 "$ROOT/tests/bench/gen_base.py" "$BASE_ROWS")
echo "==> 导入"
start
OUT="$(BENCH_CSV="$WORKDIR/base.csv" python3 tests/bench/import_base.py)"
echo "$OUT" | cut -c1-200
DS="$(echo "$OUT" | grep -o "ds_[0-9]*" | head -1)"
[ -n "$DS" ] || { echo "没有取到数据集 id"; exit 1; }
rm -f "$WORKDIR/base.csv"
kill "$SRV"; wait "$SRV" 2>/dev/null || true; SRV=
if [ "$TARGET_ROWS" -gt "$BASE_ROWS" ]; then
  echo "==> 放大到 $TARGET_ROWS 行"
  "$INFLATE" "$LEDGER_DB" "$DS" "$TARGET_ROWS"
fi
start
echo "==> 混合负载 ${SOAK_MINUTES} 分钟（$WORKERS 并发）"
set +e
python3 tests/bench/soak.py "$SRV" "$SOAK_MINUTES" "$WORKERS" "$RESULT"
soak=$?
echo "==> 与上次结果对比"
python3 tests/bench/compare.py "$RESULT" "${PREV:-/nonexistent}"
cmp=$?
set -e
echo "数据文件 $(du -sh "$LEDGER_DB" | cut -f1)；服务日志里 ERROR / panic 的行数："
grep -ciE "error|panic" "$WORKDIR/server.log" || true
[ "$soak" -eq 0 ] && [ "$cmp" -eq 0 ]
