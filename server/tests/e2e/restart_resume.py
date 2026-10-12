"""N20：服务重启后，被打断的 AI 回答可以「继续回答」（需要真的杀掉并重启服务，所以不放进 run.sh 的批量回归，单独跑）

    python3 tests/e2e/restart_resume.py        （源码目录先 cargo build；也可用 LEDGER_BIN 指定程序）

流程：模拟模型第一轮要求调用一个工具、第二轮故意卡住 → 此时 kill -9 服务 → 用同一个数据库重启 →
会话应显示「被中断」、没有在回答 → 继续回答，模型收到的是「提问 + 工具调用 + 工具结果」，没有重复的提问 → 回答完成后不再是中断状态。
"""
import gzip
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
PORT, GW_PORT, LLM_PORT = 18797, 19011, 19012
os.environ["LEDGER_E2E_BASE"] = f"http://127.0.0.1:{PORT}/api"
os.environ["LEDGER_E2E_GATEWAY"] = f"http://127.0.0.1:{GW_PORT}"
sys.path.insert(0, HERE)
from lib import A, call, check, finish  # noqa: E402

seen = []        # 模型收到的带工具的请求体
hang = threading.Event()


class Llm(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
        seen.append(body)
        n = len(seen)
        if n == 1:
            msg = {"role": "assistant", "content": "", "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "write_formula", "arguments": json.dumps({"cell": "E1", "formula": "=1+1", "explanation": "示例"})}}]}
        elif n == 2:
            hang.wait(60)            # 服务被杀掉时这个请求就悬空了
            return
        else:
            msg = {"role": "assistant", "content": "继续后的答复"}
        data = json.dumps({"choices": [{"index": 0, "message": msg}]}, ensure_ascii=False).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


llm = ThreadingHTTPServer(("127.0.0.1", LLM_PORT), Llm)
llm.daemon_threads = True
threading.Thread(target=llm.serve_forever, daemon=True).start()

tmp = tempfile.mkdtemp()
binary = os.environ.get("LEDGER_BIN") or os.path.join(ROOT, "target/debug/gridlake")
gw = subprocess.Popen([sys.executable, os.path.join(HERE, "mock_gateway.py"), str(GW_PORT)])


def start_server():
    env = dict(os.environ, LEDGER_AI_BASE_URL=f"http://127.0.0.1:{LLM_PORT}/v1", LEDGER_AI_MODEL="mock-model", LEDGER_AI_STREAM="0",
               LEDGER_AUTH_MODE="gateway", LEDGER_AUTH_URL=f"http://127.0.0.1:{GW_PORT}/current_user",
               LEDGER_ADDR=f"127.0.0.1:{PORT}", LEDGER_DB=os.path.join(tmp, "data/ledger.duckdb"), LEDGER_BACKUP_AT="off")
    p = subprocess.Popen([binary], cwd=ROOT, env=env, stdout=open(os.path.join(tmp, "server.log"), "a"), stderr=subprocess.STDOUT)
    for _ in range(100):
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{PORT}/api/health", timeout=1).read()
            return p
        except Exception:
            time.sleep(0.2)
    raise SystemExit("服务没有启动")


srv = start_server()
try:
    call("GET", "/ledger/me", A)
    s, r = call("POST", "/ledger/workbooks?name=resume", A, raw=gzip.compress(json.dumps({"sheetOrder": ["s1"], "sheets": {"s1": {"id": "s1", "name": "S", "cellData": {}}}}).encode())); wb = r["data"]["id"]
    s, r = call("POST", "/ledger/ai/threads", A, body={"workbookId": wb}); tid = r["data"]["id"]
    # 提问在后台连接里发出，卡在模型的第二次请求上
    def ask():
        try:
            call("POST", f"/ledger/ai/threads/{tid}/chat", A, body={"message": "给我一个公式", "context": {}}, binary=True)
        except Exception:
            pass                     # 服务被杀掉，连接中断是预期的
    threading.Thread(target=ask, daemon=True).start()
    for _ in range(100):
        if len(seen) >= 2:
            break
        time.sleep(0.1)
    check("模型已收到第二次请求（工具已执行，正在等模型）", len(seen) >= 2, len(seen))
    s, r = call("GET", f"/ledger/ai/threads/{tid}", A)
    check("杀服务前：正在回答，不算中断", r["data"]["running"] is True and r["data"]["interrupted"] is False, r["data"])
    srv.kill(); srv.wait()
    hang.set()
    srv = start_server()
    s, r = call("GET", f"/ledger/ai/threads/{tid}", A)
    d = r["data"]
    check("重启后：没有在回答，标记为被中断", d["running"] is False and d["interrupted"] is True, (d["running"], d["interrupted"]))
    check("已有的提问、工具调用和建议都保住了", [i["role"] for i in d["items"]] == ["user", "tool"] and len(d["actions"]) == 1, [i["role"] for i in d["items"]])
    n = len(seen)
    s, raw = call("POST", f"/ledger/ai/threads/{tid}/chat", A, body={"resume": True, "context": {}}, binary=True)
    text = raw.decode() if isinstance(raw, bytes) else str(raw)
    check("继续回答：收到 start 与 done 事件", s == 200 and '"type":"start"' in text and '"type":"done"' in text and "继续后的答复" in text, text[:300])
    msgs = seen[n]["messages"]
    roles = [m["role"] for m in msgs]
    check("模型收到：系统提示 + 提问 + 工具调用 + 工具结果，没有重复提问", roles == ["system", "user", "assistant", "tool"], roles)
    check("工具结果对应上了调用", msgs[2]["tool_calls"][0]["id"] == msgs[3]["tool_call_id"] == "call_1", msgs[2:])
    s, r = call("GET", f"/ledger/ai/threads/{tid}", A)
    d = r["data"]
    check("回答完成：不再是中断状态，只有一条提问", d["interrupted"] is False and [i["role"] for i in d["items"]] == ["user", "tool", "assistant"] and d["items"][-1]["text"] == "继续后的答复", d["items"])
    s, raw = call("POST", f"/ledger/ai/threads/{tid}/chat", A, body={"resume": True, "context": {}}, binary=True)
    check("再点继续会被拒绝", s == 400, (s, raw))
finally:
    srv.kill()
    gw.kill()
    llm.shutdown()
finish()
