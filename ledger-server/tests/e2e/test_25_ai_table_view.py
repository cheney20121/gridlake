"""AI 数据表视图建议（set_table_view）

用脚本化的模拟模型驱动 AI 助手：模型调用 set_table_view，服务端核对列名与筛选值、
给出命中行数与前几行预览，生成一张 table_view 建议。检查：
建议内容（只带给出的项）、预览（命中 / 总行数、列、行）、校验错误（列不存在、一行都没命中、什么都没改、
排序列不存在、数据集不存在）、系统提示与工具列表。应用与撤销发生在浏览器里（见前端界面测试）。
"""
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from lib import *  # noqa: F403

PORT = int(os.environ.get("LEDGER_E2E_LLM_PORT", "19002"))
script = []
seen = []


class Llm(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
        step = {"content": "- 摘要"} if "tools" not in body else (script.pop(0) if script else {"content": "（脚本用完）"})
        if "tools" in body:
            seen.append(body)
        msg = {"role": "assistant", "content": step.get("content", "")}
        calls = [{"id": f"call_{len(seen)}_{i}", "type": "function", "function": {"name": n, "arguments": json.dumps(a, ensure_ascii=False)}} for i, (n, a) in enumerate(step.get("tool_calls", []))]
        if calls:
            msg["tool_calls"] = calls
        data = json.dumps({"choices": [{"index": 0, "message": msg}]}, ensure_ascii=False).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


srv = ThreadingHTTPServer(("127.0.0.1", PORT), Llm)
threading.Thread(target=srv.serve_forever, daemon=True).start()

for t in (A, Bo): call("GET", "/ledger/me", t)
boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("订单.csv", "下单时间,店铺,金额\n2026-09-01 10:00:00,A,10\n2026-09-02 10:00:00,B,20\n2026-10-01 10:00:00,A,30\n"), ctype=f"multipart/form-data; boundary={boundary}")
p = r["data"]
s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": "create", "name": "订单", "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]]})
check("导入订单数据集", s == 200, (s, r))

wbj = gzip.compress(json.dumps({"sheetOrder": ["s1"], "sheets": {"s1": {"id": "s1", "name": "S", "cellData": {}}}}).encode())
s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("视图测试"), A, raw=wbj); wb = r["data"]["id"]
s, r = call("POST", "/ledger/ai/threads", A, body={"workbookId": wb}); check("创建会话", s == 200, (s, r)); tid = r["data"]["id"]


def ask(text, args):
    script.extend([{"tool_calls": [("set_table_view", args)]}, {"content": "好的。"}])
    s, r = call("POST", f"/ledger/ai/threads/{tid}/chat", A, body={"message": text, "context": {}}, binary=True)
    return s


def detail():
    s, r = call("GET", f"/ledger/ai/threads/{tid}", A)
    return r["data"]


def last_tool_content():
    for m in reversed(seen[-1]["messages"]):
        if m["role"] == "tool":
            return m["content"]
    return ""


BASE = {"table": "订单表", "dataset": "订单"}

# 筛选 + 排序：只带给出的项，预览有命中行数
ask("只看店铺A，按金额降序", {**BASE, "filters": [{"op": "in", "column": "店铺", "values": ["A"]}], "sort": [{"column": "金额", "desc": True}]})
names = [t["function"]["name"] for t in seen[0]["tools"]]
check("模型工具列表含 set_table_view", "set_table_view" in names, names)
d = detail()
acts = [a for a in d["actions"] if a["kind"] == "table_view"]
check("生成一条 table_view 建议", len(acts) == 1, [a["kind"] for a in d["actions"]])
pl = acts[0]["payload"]
check("建议带数据表名与数据集", pl["table"] == "订单表" and pl["datasetName"] == "订单", pl)
check("给了的项带上：筛选与排序", pl["filters"][0]["values"] == ["A"] and pl["sort"][0]["desc"] is True, pl)
check("没给的项不带（保持不变）", "fields" not in pl and "keyword" not in pl, pl)
check("预览：命中 2 行 / 共 3 行", pl["preview"]["note"].startswith("命中 2 行 / 共 3 行"), pl["preview"])
check("预览：按金额降序，第一行金额是 30", len(pl["preview"]["rows"]) == 2 and pl["preview"]["rows"][0][-1] in ("30", "30.00"), pl["preview"]["rows"])
check("标题说明做了什么", acts[0]["title"].startswith("订单表：") and "排序" in acts[0]["title"], acts[0]["title"])
tool_items = [i for i in d["items"] if i["role"] == "tool"]
check("工具调用记录显示摘要", "订单表" in tool_items[-1]["summary"], tool_items[-1])

# 显示列 + 取消排序 + 清除筛选
ask("只显示店铺和金额，取消排序", {**BASE, "fields": ["店铺", "金额"], "sort": [], "filters": []})
pl = [a for a in detail()["actions"] if a["kind"] == "table_view"][-1]["payload"]
check("fields / 清空的 sort、filters 都带上", pl["fields"] == ["店铺", "金额"] and pl["sort"] == [] and pl["filters"] == [], pl)
check("预览只含选定的列", pl["preview"]["header"] == ["店铺", "金额"] and pl["preview"]["note"].startswith("命中 3 行 / 共 3 行"), pl["preview"])

# 只给排序时，预览提示不含现有筛选
ask("按金额升序", {**BASE, "sort": [{"column": "金额"}]})
pl = [a for a in detail()["actions"] if a["kind"] == "table_view"][-1]["payload"]
check("没给 filters 时预览注明不含现有筛选", "不含表上现有的筛选" in pl["preview"]["note"], pl["preview"]["note"])


def bad(label, args, expect):
    n = len([a for a in detail()["actions"] if a["kind"] == "table_view"])
    ask(label, args)
    msg_ = last_tool_content()
    check(label, expect in msg_ and len([a for a in detail()["actions"] if a["kind"] == "table_view"]) == n, msg_[:200])


bad("筛选列不存在", {**BASE, "filters": [{"op": "in", "column": "不存在的列", "values": ["A"]}]}, "没有列")
bad("排序列不存在", {**BASE, "sort": [{"column": "不存在的列"}]}, "没有列")
bad("显示列不存在", {**BASE, "fields": ["店铺", "不存在的列"]}, "没有列")
bad("筛选值写错，一行都没命中", {**BASE, "filters": [{"op": "in", "column": "店铺", "values": ["Z店"]}]}, "一行都没有命中")
bad("什么都没改", dict(BASE), "至少给一项")
bad("数据集不存在", {"table": "订单表", "dataset": "没有这个", "sort": [{"column": "金额"}]}, "没有数据集")
bad("缺少数据表名", {"table": " ", "dataset": "订单", "sort": [{"column": "金额"}]}, "数据表名称")

# 系统提示说明何时用
sysmsg = seen[0]["messages"][0]["content"]
check("系统提示说明数据表视图的用法", "set_table_view" in sysmsg, sysmsg[-400:])

srv.shutdown()
finish()
