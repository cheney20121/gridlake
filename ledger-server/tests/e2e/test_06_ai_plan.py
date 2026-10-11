"""N9 AI 方案卡（多步执行）

用脚本化的模拟模型（OpenAI 兼容接口）驱动 AI 助手：模型调用 propose_plan，服务端逐步校验并合成一张方案卡；
检查校验（第几步有问题、步骤数、重复写入、不允许的工具）、方案卡内容、状态流转（待处理 → 已应用 → 已撤销）与撤销信息的保存。
应用与撤销本身发生在浏览器里（见前端界面测试）。
"""
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from lib import *  # noqa: F403

PORT = int(os.environ.get("LEDGER_E2E_LLM_PORT", "19002"))
script = []      # 依次返回的模型回复：{"content": ..., "tool_calls": [(name, args_dict)]}
seen = []        # 收到的请求体


class Llm(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
        # 不带工具的调用是服务端在整理较早对话的摘要（超过 6 轮后触发），不占用脚本
        step = {"content": "- 摘要：用户让助手生成过若干方案"} if "tools" not in body else (script.pop(0) if script else {"content": "（脚本用完）"})
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
s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": "create", "name": "订单", "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
check("导入订单数据集", s == 200, (s, r))

wbj = gzip.compress(json.dumps({"sheetOrder": ["s1"], "sheets": {"s1": {"id": "s1", "name": "S", "cellData": {}}}}).encode())
s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("方案测试"), A, raw=wbj); wb = r["data"]["id"]
s, r = call("POST", "/ledger/ai/threads", A, body={"workbookId": wb}); check("创建会话", s == 200, (s, r)); tid = r["data"]["id"]

PIVOT = {"dataset": "订单", "rows": [{"column": "店铺"}], "values": [{"agg": "sum", "column": "金额"}]}
CHART = {"title": "各店铺销售额", "type": "column", "dataset": "订单", "x": {"column": "店铺"}, "values": [{"agg": "sum", "column": "金额"}]}
CALC = {"dataset": "订单", "name": "金额x2", "formula": "=[金额]*2"}
FORMULA = {"cell": "E1", "formula": "=SUM(B2:B4)", "explanation": "合计"}

def ask(text, steps):
    """脚本：第一轮调用 propose_plan（steps 为 (标题, 步骤列表)），第二轮给出文字回答"""
    title, plan = steps
    script.extend([{"tool_calls": [("propose_plan", {"title": title, "steps": plan})]}, {"content": "已生成方案，请预览后应用。"}])
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

# 模型能看到这个工具，系统提示也提到了它
ask("按店铺做销售透视表并配一张图", ("各店铺销售分析", [{"tool": "create_pivot", "args": PIVOT}, {"tool": "create_chart", "args": CHART}]))
names = [t["function"]["name"] for t in seen[0]["tools"]]
check("模型工具列表含 propose_plan 且限定步数", "propose_plan" in names and "find_anomalies" in names, names)
check("系统提示说明何时用方案卡", "propose_plan" in seen[0]["messages"][0]["content"], seen[0]["messages"][0]["content"][-300:])

d = detail()
plans = [a for a in d["actions"] if a["kind"] == "plan"]
check("生成一张方案卡（而不是两张零散建议）", len(plans) == 1 and len(d["actions"]) == 1, [a["kind"] for a in d["actions"]])
plan = plans[0]
check("方案卡标题与步数", plan["title"] == "方案「各店铺销售分析」（2 步）" and plan["status"] == "proposed", plan["title"])
steps = plan["payload"]["steps"]
check("步骤顺序与类型：透视表 → 图表", [x["kind"] for x in steps] == ["pivot", "chart"], steps)
check("每步都带预览（与单独建议一致）", steps[0]["preview"]["header"] and steps[0]["preview"]["rows"], steps[0].get("preview"))
tool_items = [i for i in d["items"] if i["role"] == "tool"]
check("工具调用记录显示步骤摘要", "共 2 步" in tool_items[-1]["summary"] and "透视表 → 图表" in tool_items[-1]["summary"], tool_items[-1])
check("模型收到的结果提示它别重复列参数", "已生成方案卡" in last_tool_content() or "已生成方案卡" in json.dumps(seen[1]["messages"], ensure_ascii=False), seen[1]["messages"][-1])

# 四种步骤都能进方案
n0 = len(detail()["actions"])
ask("再来一个", ("全家桶", [{"tool": "add_calc_column", "args": CALC}, {"tool": "write_formula", "args": FORMULA}, {"tool": "create_pivot", "args": PIVOT}, {"tool": "create_chart", "args": CHART}]))
d = detail(); big = [a for a in d["actions"] if a["kind"] == "plan"][-1]
check("四类步骤可混合：calc / formula / pivot / chart", [x["kind"] for x in big["payload"]["steps"]] == ["calc", "formula", "pivot", "chart"], big["payload"]["steps"])

# 校验：错误指明第几步，不产生方案卡
def bad(label, plan_steps, expect, title="坏方案"):
    n = len(detail()["actions"])
    ask(label, (title, plan_steps))
    msg = last_tool_content()
    check(label, expect in msg and len(detail()["actions"]) == n, msg[:200])

bad("第 2 步引用不存在的列", [{"tool": "create_pivot", "args": PIVOT}, {"tool": "create_chart", "args": {**CHART, "x": {"column": "不存在的列"}}}], "第 2 步")
bad("只有 1 步不是方案", [{"tool": "create_pivot", "args": PIVOT}], "2~5 步")
bad("超过 5 步", [{"tool": "create_pivot", "args": PIVOT}] * 6, "2~5 步")
bad("方案里不能嵌套别的工具", [{"tool": "create_pivot", "args": PIVOT}, {"tool": "run_sql", "args": {"sql": "select 1"}}], "只能是 create_pivot")
bad("两步写同一个单元格", [{"tool": "write_formula", "args": FORMULA}, {"tool": "write_formula", "args": {**FORMULA, "formula": "=1"}}], "同一个位置")
bad("两步建同名计算列", [{"tool": "add_calc_column", "args": CALC}, {"tool": "add_calc_column", "args": CALC}], "同名计算列")
bad("后一步引用前一步新建的计算列", [{"tool": "add_calc_column", "args": CALC}, {"tool": "create_pivot", "args": {**PIVOT, "values": [{"agg": "sum", "column": "金额x2"}]}}], "第 2 步")
bad("缺少方案名称", [{"tool": "create_pivot", "args": PIVOT}, {"tool": "create_chart", "args": CHART}], "名称", title=" ")

# 状态流转与撤销信息
aid = plan["id"]
undo = {"steps": [{"kind": "pivot", "sheetId": "sheet-9"}, {"kind": "chart", "chartId": "c1", "sheetId": "s1"}]}
s, r = call("POST", f"/ledger/ai/actions/{aid}", A, body={"status": "undone", "result": "x"}); check("待处理的不能直接撤销", s == 400, (s, r))
s, r = call("POST", f"/ledger/ai/actions/{aid}", A, body={"status": "applied", "result": "已应用方案（2 步）", "undo": undo}); check("应用并保存撤销信息", s == 200, (s, r))
a = [x for x in detail()["actions"] if x["id"] == aid][0]
check("撤销信息随会话返回（刷新页面后仍能撤销）", a["status"] == "applied" and a["undo"] == undo and a["result"] == "已应用方案（2 步）", a)
s, r = call("POST", f"/ledger/ai/actions/{aid}", A, body={"status": "applied", "result": "再来一次"}); check("不能重复应用", s == 400 and "已应用" in msg((s, r)), (s, r))
s, r = call("POST", f"/ledger/ai/actions/{aid}", A, body={"status": "dismissed"}); check("应用后不能忽略", s == 400, (s, r))
s, r = call("POST", f"/ledger/ai/actions/{aid}", A, body={"status": "undone", "result": "已撤销方案（2 步）"}); check("撤销", s == 200, (s, r))
a = [x for x in detail()["actions"] if x["id"] == aid][0]
check("状态为已撤销，撤销信息保留（审计）", a["status"] == "undone" and a["undo"] == undo and a["result"] == "已撤销方案（2 步）", a)
s, r = call("POST", f"/ledger/ai/actions/{aid}", A, body={"status": "undone"}); check("不能重复撤销", s == 400, (s, r))
s, r = call("POST", f"/ledger/ai/actions/{aid}", A, body={"status": "weird"}); check("未知状态被拒绝", s == 400, (s, r))
s, r = call("POST", "/ledger/ai/actions/nope", A, body={"status": "applied"}); check("建议不存在", s == 400 and "不存在" in msg((s, r)), (s, r))
other = [x for x in detail()["actions"] if x["kind"] == "plan"][1]["id"]
s, r = call("POST", f"/ledger/ai/actions/{other}", A, body={"status": "dismissed"}); check("方案可以整体忽略", s == 200, (s, r))
srv.shutdown()
finish()
