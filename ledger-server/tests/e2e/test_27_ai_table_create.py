"""AI 新建数据表建议（create_data_table）

用脚本化的模拟模型驱动 AI 助手：模型调用 create_data_table，服务端核对数据集、列名、筛选值、表名与模式，
给出命中行数与前几行预览，生成一张 table_create 建议。应用与撤销发生在浏览器里（见前端界面测试）。
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
s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("新建数据表测试"), A, raw=wbj); wb = r["data"]["id"]
s, r = call("POST", "/ledger/ai/threads", A, body={"workbookId": wb}); check("创建会话", s == 200, (s, r)); tid = r["data"]["id"]


def ask(text, args):
    script.extend([{"tool_calls": [("create_data_table", args)]}, {"content": "好的。"}])
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



def tables():
    return [a for a in detail()["actions"] if a["kind"] == "table_create"]


# 最简：只给数据集 → 浮动表格、全部列、不带筛选
ask("把订单明细放成一张表", {"dataset": "订单"})
names = [t["function"]["name"] for t in seen[0]["tools"]]
check("模型工具列表含 create_data_table", "create_data_table" in names, names)
acts = tables()
check("生成一条 table_create 建议", len(acts) == 1, [a["kind"] for a in detail()["actions"]])
pl = acts[0]["payload"]
check("默认浮动表格，没给的项不带", pl["mode"] == "float" and "name" not in pl and "filters" not in pl and "fields" not in pl, pl)
check("预览：命中 3 行 / 共 3 行，列与行", pl["preview"]["note"].startswith("浮动表格，命中 3 行 / 共 3 行") and len(pl["preview"]["rows"]) == 3 and len(pl["preview"]["header"]) == 3, pl["preview"])
check("预览说明插入位置", "插入到当前选中的单元格处" in pl["preview"]["note"], pl["preview"]["note"])
check("标题带默认表名与数据集", acts[0]["title"].startswith("新建数据表「订单表」：数据集「订单」"), acts[0]["title"])

# 带表名、筛选、排序、显示列、铺到单元格
ask("店铺A的订单放进单元格", {"dataset": "订单", "name": "A店订单", "mode": "cells", "fields": ["店铺", "金额"], "filters": [{"op": "in", "column": "店铺", "values": ["A"]}], "sort": [{"column": "金额", "desc": True}]})
pl = tables()[-1]["payload"]
check("表名、模式、视图项都带上", pl["name"] == "A店订单" and pl["mode"] == "cells" and pl["fields"] == ["店铺", "金额"] and pl["filters"][0]["values"] == ["A"] and pl["sort"][0]["desc"] is True, pl)
check("预览：命中 2 行，只含选定的列，按金额降序", pl["preview"]["header"] == ["店铺", "金额"] and pl["preview"]["note"].startswith("铺到单元格，命中 2 行 / 共 3 行") and pl["preview"]["rows"][0][1] in ("30", "30.00"), pl["preview"])
check("标题说明做了什么", tables()[-1]["title"] == "新建数据表「A店订单」：数据集「订单」；筛选 店铺=A；排序 金额降序；显示 2 列", tables()[-1]["title"])


def bad(label, args, expect):
    n = len(tables())
    ask(label, args)
    msg_ = last_tool_content()
    check(label, expect in msg_ and len(tables()) == n, msg_[:200])


bad("筛选列不存在", {"dataset": "订单", "filters": [{"op": "in", "column": "不存在的列", "values": ["A"]}]}, "没有列")
bad("显示列不存在", {"dataset": "订单", "fields": ["不存在的列"]}, "没有列")
bad("排序列不存在", {"dataset": "订单", "sort": [{"column": "不存在的列"}]}, "没有列")
bad("筛选值写错，一行都没命中", {"dataset": "订单", "filters": [{"op": "in", "column": "店铺", "values": ["Z店"]}]}, "一行都没有命中")
bad("数据集不存在", {"dataset": "没有这个"}, "没有数据集")
bad("表名含公式里的特殊字符", {"dataset": "订单", "name": "A[1]"}, "不能包含")
bad("表名太长", {"dataset": "订单", "name": "长" * 41}, "最长 40 个字")
bad("未知模式", {"dataset": "订单", "mode": "grid"}, "没有模式")

# 单元格模式最多 1 万行：10001 行的数据集铺到单元格被拒绝，浮动表格可以；收紧筛选后命中 ≤ 1 万行就能铺
big = "编号,店铺,金额\n" + "".join(f"B{i},{'X' if i % 2 else 'Y'},{i}\n" for i in range(10001))
s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("大表.csv", big), ctype=f"multipart/form-data; boundary={boundary}")
p2 = r["data"]
s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p2["token"], "mode": "create", "name": "大表", "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p2["columns"]]})
check("导入 10001 行的大表", s == 200, (s, r))
bad("命中超过 1 万行不能铺到单元格", {"dataset": "大表", "mode": "cells"}, "超过单元格模式最多 10,000 行的上限")
ask("大表放成浮动表格", {"dataset": "大表"})
pl = tables()[-1]["payload"]
check("同样的数据集用浮动表格可以，命中行数带千分位", pl["mode"] == "float" and pl["preview"]["note"].startswith("浮动表格，命中 10,001 行 / 共 10,001 行"), pl["preview"]["note"])
ask("只铺 X 店", {"dataset": "大表", "mode": "cells", "filters": [{"op": "in", "column": "店铺", "values": ["X"]}]})
pl = tables()[-1]["payload"]
check("收紧筛选后命中 5000 行可以铺到单元格", pl["mode"] == "cells" and "命中 5,000 行" in pl["preview"]["note"], pl["preview"]["note"])

sysmsg = seen[0]["messages"][0]["content"]
check("系统提示说明何时新建数据表", "create_data_table" in sysmsg and "set_table_view" in sysmsg, sysmsg[-600:])

srv.shutdown()
finish()
