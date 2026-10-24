#!/usr/bin/env bash
# 端到端回归测试：每个 test_*.py 用全新的临时数据库启动一个服务（登录校验指向模拟网关），跑完即清理。
#   tests/e2e/run.sh              全部
#   tests/e2e/run.sh 02 03        只跑文件名里含 02 / 03 的
# 环境变量：LEDGER_E2E_LLM_PORT（模拟模型端口，默认 19002）、LEDGER_BIN 服务程序（默认：安装包里用 bin/gridlake；源码目录先 cargo build 再用 target/debug/gridlake）、
#           LEDGER_E2E_PORT（默认 18787）、LEDGER_E2E_GATEWAY_PORT（默认 19001）
# 只依赖 bash、python3、curl；用临时目录里的数据库，不碰生产数据。装好安装包后可以直接用它验收：
#   cd /opt/gridlake && tests/e2e/run.sh
set -uo pipefail
cd "$(dirname "$0")/../.."

PORT="${LEDGER_E2E_PORT:-18787}"
GW_PORT="${LEDGER_E2E_GATEWAY_PORT:-19001}"
BIN="${LEDGER_BIN:-}"
if [ -z "$BIN" ]; then
  if [ -x bin/gridlake ]; then
    BIN=bin/gridlake                 # 安装包 / 安装目录里：测已安装的程序
  else
    cargo build || exit 2                 # 源码目录：先增量编译，避免用到旧的程序
    BIN=target/debug/gridlake
  fi
fi
[ -x "$BIN" ] || { echo "找不到服务程序：$BIN"; exit 2; }
echo "程序：$BIN（$("$BIN" version 2>&1 | head -1)）"
command -v python3 >/dev/null && command -v curl >/dev/null || { echo "需要 python3 与 curl"; exit 2; }
LLM_PORT="${LEDGER_E2E_LLM_PORT:-19002}"
export LEDGER_E2E_BASE="http://127.0.0.1:${PORT}/api" LEDGER_E2E_GATEWAY="http://127.0.0.1:${GW_PORT}" LEDGER_E2E_LLM_PORT="$LLM_PORT"

TMP="$(mktemp -d)"
export LEDGER_E2E_TMP="$TMP"
# 被测的服务程序（测试 gridlake open 这类子命令时自己启动）
export LEDGER_E2E_BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
# 一个最小的前端目录：测试里写 LEDGER_WEB_DIR=@WEB@ 就由服务直接提供这个页面（用来检查页面的响应头）
mkdir -p "$TMP/web" && printf '<!doctype html><title>e2e</title>\n' > "$TMP/web/index.html"
PIDS=()
cleanup() { for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null; done; rm -rf "$TMP"; }
trap cleanup EXIT

wait_http() { for _ in $(seq 1 50); do curl -s -o /dev/null "$1" && return 0; sleep 0.2; done; return 1; }

# 测试用的内部 CA 与服务器证书（TLS 测试用：对象存储连接器通过 LEDGER_AUTH_CA_FILE 信任它）。没有 openssl 时跳过，依赖它的测试会自行跳过
TLS_DIR="$TMP/tls"
if command -v openssl >/dev/null && mkdir -p "$TLS_DIR" && (
  cd "$TLS_DIR" &&
  openssl req -x509 -newkey rsa:2048 -nodes -keyout ca.key -out ca.pem -days 30 -subj "/CN=Gridlake Test CA" \
    -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" &&
  openssl req -newkey rsa:2048 -nodes -keyout server.key -out server.csr -subj "/CN=localhost" &&
  printf 'subjectAltName=IP:127.0.0.1,DNS:localhost\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n' >ext.cnf &&
  openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out server.pem -days 30 -extfile ext.cnf
) >"$TMP/tls.log" 2>&1; then
  export LEDGER_E2E_TLS_DIR="$TLS_DIR"
  CA_ENV="LEDGER_AUTH_CA_FILE=$TLS_DIR/ca.pem"
else
  CA_ENV=""
  echo "（没有生成测试证书，TLS 相关的测试会跳过）"
fi

failed=()
ran=0
for t in tests/e2e/test_*.py; do
  if [ "$#" -gt 0 ]; then
    match=0; for k in "$@"; do [[ "$t" == *"$k"* ]] && match=1; done
    [ "$match" = 1 ] || continue
  fi
  ran=$((ran + 1))
  echo "━━ $(basename "$t")"
  python3 tests/e2e/mock_gateway.py "$GW_PORT" & GW=$!
  rm -rf "$TMP/data"
  # 测试文件第一行 "# env: A=1 B=2" 可以给这个测试的服务加环境变量（如 test_09 把重计算并发限制为 1）
  EXTRA_ENV="$(sed -n '1s/^# env: *//p' "$t")"
  EXTRA_ENV="${EXTRA_ENV//@WEB@/$TMP/web}"
  # @TMP@：这次运行的临时目录（测试里用环境变量 LEDGER_E2E_TMP 拿到同一个目录，比如往收件目录里放文件）
  EXTRA_ENV="${EXTRA_ENV//@TMP@/$TMP}"
  # AI 助手指向本机的模拟模型（只有用到 AI 的测试会在这个端口上起模拟模型，见 test_06）
  # 测试文件里的环境变量放在最后，可以覆盖默认值（如 test_30 改用内置账号 LEDGER_AUTH_MODE=local）
  env LEDGER_AI_BASE_URL="http://127.0.0.1:${LLM_PORT}/v1" LEDGER_AI_MODEL=mock-model LEDGER_AI_STREAM=0 \
    LEDGER_AUTH_MODE=gateway LEDGER_AUTH_URL="http://127.0.0.1:${GW_PORT}/current_user" LEDGER_PACKS=ecommerce \
    LEDGER_ADDR="127.0.0.1:${PORT}" LEDGER_DB="$TMP/data/ledger.duckdb" LEDGER_BACKUP_AT=off $CA_ENV $EXTRA_ENV \
    "$BIN" >"$TMP/server.log" 2>&1 & SRV=$!
  PIDS=("$GW" "$SRV")
  if wait_http "http://127.0.0.1:${PORT}/api/health" && wait_http "http://127.0.0.1:${GW_PORT}/webconfig"; then
    python3 "$t" || failed+=("$(basename "$t")")
  else
    echo "服务没有启动："; tail -5 "$TMP/server.log"; failed+=("$(basename "$t")（启动失败）")
  fi
  kill "$SRV" "$GW" 2>/dev/null; wait "$SRV" "$GW" 2>/dev/null
  PIDS=()
done

echo
if [ "${#failed[@]}" -gt 0 ]; then echo "失败：${failed[*]}"; exit 1; fi
[ "$ran" -gt 0 ] || { echo "没有匹配的测试"; exit 2; }
echo "端到端测试全部通过（$ran 个文件）"
