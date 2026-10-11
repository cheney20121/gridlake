"""AI 方案卡里的数据表工具（create_data_table / set_table_view / set_column_format）

用脚本化的模拟模型驱动 AI 助手：模型调用 propose_plan，步骤里用数据表工具。检查：逐步校验（指明第几步）、
调整本方案前面新建的数据表（可以，且新建要排在前面）、一个方案最多新建一张数据表、同一张表同一项不能被两步调整、
save_query_as_dataset 不能进方案。应用与撤销发生在浏览器里（见前端界面测试）。
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
s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("方案数据表测试"), A, raw=wbj); wb = r["data"]["id"]
s, r = call("POST", "/ledger/ai/threads", A, body={"workbookId": wb}); check("创建会话", s == 200, (s, r)); tid = r["data"]["id"]


def ask(text, title, plan):
    script.extend([{"tool_calls": [("propose_plan", {"title": title, "steps": plan})]}, {"content": "已生成方案。"}])
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


def plans():
    return [a for a in detail()["actions"] if a["kind"] == "plan"]


CREATE = {"tool": "create_data_table", "args": {"dataset": "订单", "name": "甲店明细", "filters": [{"op": "in", "column": "店铺", "values": ["A"]}]}}
FMT = {"tool": "set_column_format", "args": {"table": "甲店明细", "dataset": "订单", "column": "金额", "numFmt": "wan", "rules": [{"op": ">", "value": 15, "style": {"color": "#C00000"}}]}}
VIEW = {"tool": "set_table_view", "args": {"table": "已有表", "dataset": "订单", "sort": [{"column": "金额", "desc": True}]}}
PIVOT = {"tool": "create_pivot", "args": {"dataset": "订单", "rows": [{"column": "店铺"}], "values": [{"agg": "sum", "column": "金额"}]}}

# 新建数据表 + 给它设列样式（同一个方案里引用刚新建的表）
ask("把甲店订单放成一张表，金额用万元显示，超过 15 的标红", "甲店明细表", [CREATE, FMT])
names = [t["function"]["name"] for t in seen[0]["tools"]]
check("模型工具列表含三个数据表工具", all(n in names for n in ["create_data_table", "set_table_view", "set_column_format"]), names)
check("系统提示说明方案里可以调整刚新建的数据表", "create_data_table + set_column_format" in seen[0]["messages"][0]["content"], seen[0]["messages"][0]["content"][-300:])
ps = plans()
check("生成一张方案卡", len(ps) == 1 and ps[0]["title"] == "方案「甲店明细表」（2 步）", [p["title"] for p in ps])
steps = ps[0]["payload"]["steps"]
check("步骤：新建数据表 → 列样式，各带预览", [x["kind"] for x in steps] == ["table_create", "column_format"] and steps[0]["preview"]["rows"] and steps[1]["preview"]["rows"], steps)
check("步骤内容与单独调用一致", steps[0]["name"] == "甲店明细" and steps[1]["table"] == "甲店明细" and steps[1]["numFmt"] == "wan", steps)
tool_items = [i for i in detail()["items"] if i["role"] == "tool"]
check("工具调用摘要写出步骤类型", "新建数据表 → 列样式" in tool_items[-1]["summary"], tool_items[-1])

# 没给表名：默认「数据集名+表」，后面的步骤用这个名字（不区分大小写）
ask("放成一张表并按金额降序", "订单明细", [{"tool": "create_data_table", "args": {"dataset": "订单"}}, {"tool": "set_table_view", "args": {"table": "订单表", "dataset": "订单", "sort": [{"column": "金额", "desc": True}]}}])
check("引用默认表名的方案可以生成", plans()[-1]["title"] == "方案「订单明细」（2 步）", last_tool_content()[:200])

# 调整已有的数据表（不在本方案里新建）+ 其它类型步骤混用
ask("混合方案", "混合", [VIEW, {"tool": "set_column_format", "args": {"table": "已有表", "dataset": "订单", "column": "店铺", "rules": [{"op": "contains", "value": "A", "style": {"bold": True}}]}}, PIVOT])
check("调整已有数据表 + 透视表可以混用", [x["kind"] for x in plans()[-1]["payload"]["steps"]] == ["table_view", "column_format", "pivot"], last_tool_content()[:200])

# 8 步是上限
ask("八步", "八步", [PIVOT] * 8)
check("8 步可以", plans()[-1]["title"] == "方案「八步」（8 步）", last_tool_content()[:200])


def bad(label, steps, expect, title="坏方案"):
    n = len(plans())
    ask(label, title, steps)
    msg_ = last_tool_content()
    check(label, expect in msg_ and len(plans()) == n, msg_[:240])


bad("引用后面才新建的表", [FMT, CREATE], "在第 2 步才新建")
bad("一个方案里新建两张表", [CREATE, {"tool": "create_data_table", "args": {"dataset": "订单", "name": "乙店明细"}}], "最多新建一张数据表")
bad("同名新建两次", [CREATE, CREATE], "新建了同名数据表")
bad("同一张表的视图被两步调整", [VIEW, {"tool": "set_table_view", "args": {"table": "已有表", "dataset": "订单", "keyword": "A"}}], "同一张数据表的视图")
bad("同一张表同一列样式被两步设置", [FMT, {"tool": "set_column_format", "args": {**FMT["args"], "numFmt": "int"}}], "同一张数据表的同一列样式")
bad("方案里不能用另存为数据集", [PIVOT, {"tool": "save_query_as_dataset", "args": {"sql": "SELECT 1 AS a", "name": "x"}}], "不能用 save_query_as_dataset")
bad("某一步有问题时指明第几步", [CREATE, {"tool": "set_column_format", "args": {"table": "甲店明细", "dataset": "订单", "column": "店铺", "rules": [{"op": ">", "value": 1, "style": {"bold": True}}]}}], "第 2 步（set_column_format）有问题")
bad("新建数据表的步骤有错", [{"tool": "create_data_table", "args": {"dataset": "订单", "filters": [{"op": "in", "column": "店铺", "values": ["Z店"]}]}}, PIVOT], "第 1 步（create_data_table）有问题")

srv.shutdown()
finish()
