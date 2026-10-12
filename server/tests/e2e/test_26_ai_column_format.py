"""AI 数据表列样式建议（set_column_format）

用脚本化的模拟模型驱动 AI 助手：模型调用 set_column_format，服务端核对列类型与规则（运算与列类型匹配、颜色只能是十六进制、
最多 8 条），统计每条规则在整个数据集里命中多少行，生成一张 column_format 建议。应用与撤销发生在浏览器里（见前端界面测试）。
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
s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("列样式测试"), A, raw=wbj); wb = r["data"]["id"]
s, r = call("POST", "/ledger/ai/threads", A, body={"workbookId": wb}); check("创建会话", s == 200, (s, r)); tid = r["data"]["id"]


def ask(text, args):
    script.extend([{"tool_calls": [("set_column_format", args)]}, {"content": "好的。"}])
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


def actions():
    return [a for a in detail()["actions"] if a["kind"] == "column_format"]


BASE = {"table": "订单表", "dataset": "订单", "column": "金额"}
RED = {"color": "#C00000", "bg": "#FDE8E8", "bold": True}

# 数字格式 + 两条规则
ask("金额用万元显示，大于 15 的标红，5 到 15 的加粗", {**BASE, "numFmt": "wan", "rules": [{"op": ">", "value": 15, "style": RED}, {"op": "between", "value": 5, "value2": 15, "style": {"bold": True}}]})
names = [t["function"]["name"] for t in seen[0]["tools"]]
check("模型工具列表含 set_column_format", "set_column_format" in names, names)
acts = actions()
check("生成一条 column_format 建议", len(acts) == 1, [a["kind"] for a in detail()["actions"]])
pl = acts[0]["payload"]
check("建议带数据表、列、数字格式、规则", pl["table"] == "订单表" and pl["column"] == "金额" and pl["numFmt"] == "wan" and len(pl["rules"]) == 2, pl)
check("规则整理成界面写法：数字、样式", pl["rules"][0] == {"op": ">", "value": 15.0, "style": {"color": "#C00000", "bg": "#FDE8E8", "bold": True}}, pl["rules"][0])
rows = pl["preview"]["rows"]
check("预览：每条规则命中行数（> 15 命中 2 行，5~15 命中 1 行）", [r[1] for r in rows] == ["2", "1"], rows)
check("预览：占整个数据集的比例", rows[0][2] == "66.67%", rows)
check("预览说明按整个数据集统计", "数据集共 3 行" in pl["preview"]["note"] and "不含表上的筛选" in pl["preview"]["note"], pl["preview"]["note"])
check("标题说明做了什么", "数字格式 万元" in acts[0]["title"] and "2 条条件格式" in acts[0]["title"], acts[0]["title"])

# 文本列的规则；只给 rules 不给 numFmt（numFmt 不带）
ask("店铺含 A 的加粗", {"table": "订单表", "dataset": "订单", "column": "店铺", "rules": [{"op": "contains", "value": "A", "style": {"bold": True}}]})
pl = actions()[-1]["payload"]
check("文本列规则命中 2 行，不带 numFmt", pl["preview"]["rows"][0][1] == "2" and "numFmt" not in pl, pl)

# 清除：空数组、default
ask("清掉金额的条件格式并恢复默认格式", {**BASE, "numFmt": "default", "rules": []})
pl = actions()[-1]["payload"]
check("清除条件格式与数字格式", pl["rules"] == [] and pl["numFmt"] == "default" and pl["preview"]["rows"] == [], pl)

# 没有命中任何行：照样出建议，但告诉模型阈值可能不对
ask("金额小于 0 的标红", {**BASE, "rules": [{"op": "negative", "style": {"color": "#C00000"}}]})
check("规则没有命中任何行时提醒确认阈值", "没有命中任何行" in last_tool_content(), last_tool_content()[:300])


def bad(label, args, expect):
    n = len(actions())
    ask(label, args)
    msg_ = last_tool_content()
    check(label, expect in msg_ and len(actions()) == n, msg_[:200])


bad("文本列不能用大于", {"table": "订单表", "dataset": "订单", "column": "店铺", "rules": [{"op": ">", "value": 1, "style": RED}]}, "只能用于金额 / 数值列")
bad("数值列不能用包含", {**BASE, "rules": [{"op": "contains", "value": "x", "style": RED}]}, "只能用于文本")
bad("文本列不能设数字格式", {"table": "订单表", "dataset": "订单", "column": "店铺", "numFmt": "wan"}, "数字格式只能用于金额 / 数值列")
bad("颜色必须是十六进制", {**BASE, "rules": [{"op": ">", "value": 1, "style": {"color": "red"}}]}, "十六进制")
bad("样式不能为空", {**BASE, "rules": [{"op": ">", "value": 1, "style": {}}]}, "至少给一项")
bad("大于需要数字", {**BASE, "rules": [{"op": ">", "style": RED}]}, "需要数字")
bad("between 下限不能大于上限", {**BASE, "rules": [{"op": "between", "value": 9, "value2": 1, "style": RED}]}, "下限不能大于上限")
bad("最多 8 条规则", {**BASE, "rules": [{"op": ">", "value": i, "style": RED} for i in range(9)]}, "最多 8 条")
bad("未知数字格式", {**BASE, "numFmt": "yi"}, "没有数字格式")
bad("列不存在", {**BASE, "column": "不存在的列", "numFmt": "int"}, "没有列")
bad("什么都没设", dict(BASE), "至少给一项")
bad("数据集不存在", {**BASE, "dataset": "没有这个", "numFmt": "int"}, "没有数据集")

sysmsg = seen[0]["messages"][0]["content"]
check("系统提示说明列样式的用法", "set_column_format" in sysmsg, sysmsg[-500:])

srv.shutdown()
finish()
