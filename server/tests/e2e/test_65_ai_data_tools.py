"""AI 助手的数据概况、数据清洗建议、窗口计算建议

用脚本化的模拟模型驱动 AI 助手：
- profile_dataset：只读，返回每列的空值 / 不同值 / 范围，并列出可能的问题（只有空白、认不出的日期、空值多）；MCP 也能调用
- propose_clean：服务端先预览（受影响行数、改前改后样例），生成一张 clean 建议；**用户确认之前不改数据**；没有要处理的行时不出建议；
  删除行必须给筛选；数据范围受限的用户不能提
- create_window_dataset：服务端先预览新列，生成一张 window 建议；用户确认之前不创建数据集；名称不能重名
应用（调清洗 / 窗口计算接口）与撤销发生在浏览器里（见前端界面测试）。
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
        msg_ = {"role": "assistant", "content": step.get("content", "")}
        calls = [{"id": f"call_{len(seen)}_{i}", "type": "function", "function": {"name": n, "arguments": json.dumps(a, ensure_ascii=False)}} for i, (n, a) in enumerate(step.get("tool_calls", []))]
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

for t in (A, Bo, R):
    call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}
boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
CSV = ("下单时间,店铺,金额\n"
       "2026-09-01 10:00:00,北京店,10\n"
       "2026-09-01 10:00:00,北京店,10\n"
       "2026-09-02 10:00:00,北京,20\n"
       "2026-09-03 10:00:00,上海店,30\n"
       "下周一,上海店,40\n"
       "2026-09-05 10:00:00,上海店,\n")
s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("体检订单.csv", CSV), ctype=f"multipart/form-data; boundary={boundary}")
p = r["data"]
s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": "create", "name": "体检订单", "columns": [{"source": c["source"], "title": c["title"], "kind": {"下单时间": "datetime", "金额": "money"}.get(c["title"], "text")} for c in p["columns"]]})
check("导入数据集", s == 200, (s, r))
D = r["data"]["datasetId"]
rows = lambda: next(d for d in call("GET", "/ledger/datasets", A)[1]["data"] if d["id"] == D)["rowCount"]
names = lambda: [d["name"] for d in call("GET", "/ledger/datasets", A)[1]["data"]]

wbj = gzip.compress(json.dumps({"sheetOrder": ["s1"], "sheets": {"s1": {"id": "s1", "name": "S", "cellData": {}}}}).encode())
def thread(tok):
    s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("AI 数据工具"), tok, raw=wbj)
    s, r = call("POST", "/ledger/ai/threads", tok, body={"workbookId": r["data"]["id"]})
    return r["data"]["id"]
tid = thread(A)


def ask(tool, args, tok=A, t=None):
    script.extend([{"tool_calls": [(tool, args)]}, {"content": "好的。"}])
    s, r = call("POST", f"/ledger/ai/threads/{t or tid}/chat", tok, body={"message": f"请{tool}", "context": {}}, binary=True)
    assert s == 200, (s, r)


def actions(kind, tok=A, t=None):
    s, r = call("GET", f"/ledger/ai/threads/{t or tid}", tok)
    return [a for a in r["data"]["actions"] if a["kind"] == kind]


def last_tool():
    for m in reversed(seen[-1]["messages"]):
        if m["role"] == "tool":
            return m["content"]
    return ""


# ── 数据概况 ──
ask("profile_dataset", {"dataset": "体检订单"})
tools = [t["function"]["name"] for t in seen[0]["tools"]]
check("模型工具列表含三个新工具", {"profile_dataset", "propose_clean", "create_window_dataset"} <= set(tools), tools)
c = last_tool()
check("概况：行数、各列空值 / 不同值 / 范围", "共 6 行" in c and "金额\t金额\t1（16.7%）\t4" in c and "上海店" in c, c[:600])
check("概况：列出认不出的日期", "认不出的日期 1" in c and "「下单时间」有 1 个认不出的日期写法" in c, c[-400:])
ask("profile_dataset", {"dataset": "体检订单", "filters": [{"op": "in", "column": "店铺", "values": ["上海店"]}], "columns": ["金额"]})
c = last_tool()
check("概况可以按筛选、只看某几列", "（按筛选）共 3 行，1 列" in c and "合计 70" in c, c[:300])

# ── 清洗建议 ──
ask("propose_clean", {"dataset": "体检订单", "op": "dedupe"})
acts = actions("clean")
check("生成一条 clean 建议", len(acts) == 1, acts)
pl = acts[0]["payload"]
check("建议带数据集、清洗参数、受影响行数", pl["datasetId"] == D and pl["op"] == {"op": "dedupe", "columns": []} and pl["affected"] == 1 and pl["deletes"], pl)
check("预览：要删的行和说明", pl["preview"]["rows"][0][:3] == ["删除", "2026-09-01 10:00:00", "北京店"] and "6 行中将删除 1 行" in pl["preview"]["note"] and "撤销" in pl["preview"]["note"], pl["preview"])
check("用户确认之前不改数据", rows() == 6, rows())

ask("propose_clean", {"dataset": "体检订单", "op": "replace", "column": "店铺", "find": "北京", "replace": "北京店", "whole": True})
pl = actions("clean")[-1]["payload"]
check("查找替换：改前改后", [r[0] for r in pl["preview"]["rows"][:2]] == ["改前", "改后"] and "北京" in pl["preview"]["rows"][0] and "北京店" in pl["preview"]["rows"][1] and pl["affected"] == 1, pl["preview"])
c = last_tool()
check("回给模型的内容说明等待用户确认", "等待用户确认" in c and "改后" in c, c[:300])

ask("propose_clean", {"dataset": "体检订单", "op": "delete_rows", "filters": [{"op": "in", "column": "店铺", "values": ["上海店"]}, {"op": "prefix", "column": "下单时间", "value": "下周"}]})
pl = actions("clean")[-1]["payload"]
check("删除筛选出的行：条件带进清洗参数（列名换成列键）", pl["op"]["op"] == "delete_rows" and len(pl["op"]["spec"]["filters"]) == 2 and pl["affected"] == 1, pl)

n = len(actions("clean"))
ask("propose_clean", {"dataset": "体检订单", "op": "delete_rows"})
check("删除行必须给筛选", "filters" in last_tool() and len(actions("clean")) == n, last_tool())
ask("propose_clean", {"dataset": "体检订单", "op": "trim"})
check("没有要处理的行：不出建议", "不用清洗" in last_tool() and len(actions("clean")) == n, last_tool())
ask("propose_clean", {"dataset": "体检订单", "op": "fill", "column": "金额", "value": "很多"})
check("填的值不合列类型：报错让模型改", "不是金额" in last_tool() and len(actions("clean")) == n, last_tool())
ask("propose_clean", {"dataset": "体检订单", "op": "sort"})
check("不认识的清洗方式", "清洗参数不正确" in last_tool(), last_tool())

# ── 窗口计算建议 ──
before = names()
ask("create_window_dataset", {"dataset": "体检订单", "name": "店铺累计", "calcs": [{"func": "running_sum", "column": "金额", "name": "累计金额"}], "partitionBy": ["店铺"], "orderBy": [{"column": "下单时间"}]})
acts = actions("window")
check("生成一条 window 建议", len(acts) == 1, [a["kind"] for a in actions("clean")])
pl = acts[0]["payload"]
check("建议带名称、窗口设置、SQL", pl["name"] == "店铺累计" and pl["spec"]["calcs"][0]["func"] == "running_sum" and pl["spec"]["partitionBy"] == ["店铺"] and "OVER" in pl["sql"], pl)
check("预览只列分组、顺序和新列", pl["preview"]["header"] == ["店铺", "下单时间", "累计金额"] and len(pl["preview"]["rows"]) == 6, pl["preview"])
check("用户确认之前不创建数据集", names() == before, names())
n = len(actions("window"))
ask("create_window_dataset", {"dataset": "体检订单", "name": "体检订单", "calcs": [{"func": "row_number", "name": "序号"}]})
check("名称不能与已有数据集重名", "已有名为「体检订单」" in last_tool() and len(actions("window")) == n, last_tool())
ask("create_window_dataset", {"dataset": "体检订单", "name": "缺列", "calcs": [{"func": "running_sum", "name": "累计"}]})
check("设置不对：报错让模型改", len(actions("window")) == n and last_tool().startswith("错误"), last_tool())

# ── 数据范围受限的用户 ──
call("PUT", f"/ledger/data-scope/datasets/{D}", R, body={"scopeColumn": "店铺", "open": False})
call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["北京店"]})
tb = thread(Bo)
ask("profile_dataset", {"dataset": "体检订单"}, tok=Bo, t=tb)
check("受限用户的概况只统计自己能看到的行", "共 2 行" in last_tool(), last_tool()[:200])
ask("propose_clean", {"dataset": "体检订单", "op": "dedupe"}, tok=Bo, t=tb)
check("受限用户不能提清洗建议", "数据范围受限" in last_tool() and not actions("clean", Bo, tb), last_tool())
ask("create_window_dataset", {"dataset": "体检订单", "name": "受限", "calcs": [{"func": "row_number", "name": "序号"}]}, tok=Bo, t=tb)
check("受限用户不能提窗口计算建议", "数据范围受限" in last_tool() and not actions("window", Bo, tb), last_tool())
call("DELETE", f"/ledger/data-scope/rules/{ids['Bob']}", R)

sysmsg = seen[0]["messages"][0]["content"]
check("系统提示说明何时体检、清洗、窗口计算", "profile_dataset" in sysmsg and "propose_clean" in sysmsg and "create_window_dataset" in sysmsg, sysmsg[-500:])

# ── MCP：数据概况 ──
s, r = call("POST", "/ledger/mcp/tokens", R, body={"name": "体检", "allowSql": False})
check("创建 MCP 令牌", s == 200, (s, r))
mcp = lambda m, p: call("POST", "/mcp", None, body={"jsonrpc": "2.0", "id": 1, "method": m, "params": p}, headers={"Authorization": f"Bearer {r['data']['secret']}"})
s, lst = mcp("tools/list", {})
mnames = [t["name"] for t in lst["result"]["tools"]]
check("MCP 有 profile_dataset，没有清洗 / 窗口计算（MCP 只读）", "profile_dataset" in mnames and "propose_clean" not in mnames and "create_window_dataset" not in mnames, mnames)
s, res = mcp("tools/call", {"name": "profile_dataset", "arguments": {"dataset": "体检订单"}})
check("MCP 调用数据概况", s == 200 and not res["result"]["isError"] and "共 6 行" in res["result"]["content"][0]["text"], res)

srv.shutdown()
finish()
