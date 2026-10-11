"""N20：AI 会话只有创建人和管理员能访问；摘要可手动编辑；没有被中断的回答不能「继续回答」

对话里有按创建人的数据范围查出的结果，所以同一个工作簿里别人的会话既不出现在列表里，也不能凭链接读取、提问、停止、删除、编辑摘要、处理其中的建议。
被服务重启打断的回答如何继续见 restart_resume.sh（需要真的重启服务）。
"""
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from lib import *  # noqa: F403

PORT = int(os.environ.get("LEDGER_E2E_LLM_PORT", "19002"))


script = []      # 依次返回的模型回复：{"content": ..., "tool_calls": [(name, args)]}；用完后一律回复「好的。」


class Llm(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        step = script.pop(0) if script else {"content": "好的。"}
        msg_ = {"role": "assistant", "content": step.get("content", "")}
        calls = [{"id": f"call_{i}", "type": "function", "function": {"name": n, "arguments": json.dumps(a, ensure_ascii=False)}} for i, (n, a) in enumerate(step.get("tool_calls", []))]
        if calls:
            msg_["tool_calls"] = calls
        data = json.dumps({"choices": [{"index": 0, "message": msg_}]}, ensure_ascii=False).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


srv = ThreadingHTTPServer(("127.0.0.1", PORT), Llm)
threading.Thread(target=srv.serve_forever, daemon=True).start()

for t in (A, Bo, R): call("GET", "/ledger/me", t)
wbj = gzip.compress(json.dumps({"sheetOrder": ["s1"], "sheets": {"s1": {"id": "s1", "name": "S", "cellData": {}}}}).encode())
s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("会话权限"), A, raw=wbj); wb = r["data"]["id"]
s, r = call("PUT", f"/ledger/workbooks/{wb}/share", A, body={"visibility": "edit"}); check("alice 把工作簿共享为可编辑", s == 200, (s, r))

s, r = call("POST", "/ledger/ai/threads", A, body={"workbookId": wb}); check("alice 新建会话", s == 200, (s, r)); ta = r["data"]["id"]
s, r = call("POST", "/ledger/ai/threads", Bo, body={"workbookId": wb, "ownerId": "u1"}); check("bob 新建会话（请求里冒充创建人无效）", s == 200, (s, r)); tb = r["data"]["id"]
script.extend([{"tool_calls": [("write_formula", {"cell": "E1", "formula": "=1+1", "explanation": "示例"})]}, {"content": "好的。"}])
s, _ = call("POST", f"/ledger/ai/threads/{ta}/chat", A, body={"message": "你好", "context": {}}, binary=True); check("alice 提问（模型给出一条公式建议）", s == 200, s)
s, _ = call("POST", f"/ledger/ai/threads/{tb}/chat", Bo, body={"message": "我是 bob", "context": {}}, binary=True); check("bob 提问", s == 200, s)

def ids(tok):
    s, r = call("GET", f"/ledger/ai/threads?workbookId={wb}", tok)
    return sorted(t["id"] for t in r["data"]) if s == 200 else (s, r)

check("alice 的列表只有自己的会话", ids(A) == [ta], ids(A))
check("bob 的列表只有自己的会话", ids(Bo) == [tb], ids(Bo))
check("管理员看到全部", ids(R) == sorted([ta, tb]), ids(R))

# 别人的会话：读 / 提问 / 停止 / 删除 / 摘要 / 建议 都是 403，并说明是谁的
def forbidden(label, method, path, tok, body=None):
    s, r = call(method, path, tok, body=body, binary=(path.endswith("/chat")))
    text = str(msg((s, r)))
    check(label, s == 403 and "Alice" in text, (s, text))

forbidden("bob 读 alice 的会话", "GET", f"/ledger/ai/threads/{ta}", Bo)
forbidden("bob 订阅 alice 会话的回答", "GET", f"/ledger/ai/threads/{ta}/events", Bo)
forbidden("bob 向 alice 的会话提问", "POST", f"/ledger/ai/threads/{ta}/chat", Bo, {"message": "偷看", "context": {}})
forbidden("bob 停止 alice 的回答", "POST", f"/ledger/ai/threads/{ta}/stop", Bo)
forbidden("bob 删除 alice 的会话", "DELETE", f"/ledger/ai/threads/{ta}", Bo)
forbidden("bob 改 alice 会话的摘要", "PUT", f"/ledger/ai/threads/{ta}/summary", Bo, {"summary": "x"})
s, r = call("GET", f"/ledger/ai/threads/{ta}", A); check("alice 自己读得到，内容没被改动", s == 200 and [i["text"] for i in r["data"]["items"] if i["role"] == "user"] == ["你好"], r)

# 建议跟着会话走
s, r = call("GET", f"/ledger/ai/threads/{ta}", A)
check("没有被中断的回答", r["data"]["interrupted"] is False, r["data"])
acts = r["data"]["actions"]; check("alice 的会话里有一条建议", len(acts) == 1 and acts[0]["status"] == "proposed", acts)
aid = acts[0]["id"]
forbidden("bob 处理 alice 会话里的建议", "POST", f"/ledger/ai/actions/{aid}", Bo, {"status": "applied", "result": "x"})
s, r = call("GET", f"/ledger/ai/threads/{ta}", A); check("建议没有被改动", r["data"]["actions"][0]["status"] == "proposed", r["data"]["actions"])
s, r = call("POST", f"/ledger/ai/actions/{aid}", R, body={"status": "dismissed"}); check("管理员可以处理", s == 200, (s, r))

# 管理员可以读，也可以删别人的
s, r = call("GET", f"/ledger/ai/threads/{ta}", R); check("管理员读 alice 的会话", s == 200, (s, r))

# 没有中断的回答不能继续
s, r = call("POST", f"/ledger/ai/threads/{ta}/chat", A, body={"resume": True, "context": {}}, binary=True)
check("没有被中断的回答时不能继续", s == 400, (s, r))
s, r = call("POST", f"/ledger/ai/threads/{ta}/chat", A, body={"message": "  ", "context": {}}, binary=True)
check("空问题仍被拒绝", s == 400, (s, r))

# 摘要手动编辑
s, r = call("PUT", f"/ledger/ai/threads/{ta}/summary", A, body={"summary": "  - 用户关心店铺 A\n- 金额单位用万  "}); check("编辑摘要", s == 200, (s, r))
s, r = call("GET", f"/ledger/ai/threads/{ta}", A); check("摘要已保存且去掉首尾空白", r["data"]["summary"] == "- 用户关心店铺 A\n- 金额单位用万", r["data"]["summary"])
s, r = call("PUT", f"/ledger/ai/threads/{ta}/summary", A, body={"summary": "字" * 4001}); check("摘要超过 4000 字被拒绝", s == 400 and "4000" in msg((s, r)), (s, r))
s, r = call("GET", f"/ledger/ai/threads/{ta}", A); check("被拒绝时摘要不变", r["data"]["summary"].startswith("- 用户关心店铺 A"), r["data"]["summary"])
s, r = call("PUT", f"/ledger/ai/threads/{ta}/summary", A, body={"summary": ""}); check("留空清除摘要", s == 200, (s, r))
s, r = call("GET", f"/ledger/ai/threads/{ta}", A); check("摘要已清除", r["data"]["summary"] is None, r["data"]["summary"])
s, r = call("PUT", f"/ledger/ai/threads/nope/summary", A, body={"summary": "x"}); check("会话不存在", s == 400 and "不存在" in msg((s, r)), (s, r))

# 删除：自己的可以；管理员可以删别人的
s, r = call("DELETE", f"/ledger/ai/threads/{ta}", A); check("alice 删除自己的会话", s == 200, (s, r))
s, r = call("DELETE", f"/ledger/ai/threads/{tb}", R); check("管理员删除 bob 的会话", s == 200, (s, r))
check("删除后列表为空", ids(R) == [], ids(R))

# 审计里有编辑摘要
time.sleep(3)
s, r = call("GET", "/ledger/audit?size=200", R)
labels = [x["action"] for x in r["data"]["items"]] if s == 200 else []
check("审计记录了编辑 AI 会话摘要", "编辑 AI 会话摘要" in labels, (s, labels[:8]))
srv.shutdown()
finish()
