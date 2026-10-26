"""推荐校验规则：按数据本身的样子推荐（不用 AI），附原因和现在不合格的行数；已有的规则不再推荐；
AI 助手的 propose_validation：不给规则时用推荐的规则，给了就按给的；生成一张 validation 建议，**用户确认之前不保存**；
默认加在已有规则后面，replace 时替换；规则写错时报错让模型改；数据范围受限的用户不能提。
应用（调保存接口）与撤销发生在浏览器里（见前端界面测试）。
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
def import_csv(name, csv, kinds):
    s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp(f"{name}.csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
    p = r["data"]
    s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": "create", "name": name, "columns": [{"source": c["source"], "title": c["title"], "kind": kinds.get(c["title"], "text")} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
    assert s == 200, (s, r)
    return r["data"]["datasetId"]

# 单号：D001 重复一次；手机号：一个位数不对、一个空；等级：「钻石」只出现一次；金额：一个负数；备注：没规律
CSV = "单号,手机号,等级,金额,备注\nD001,13800000001,银卡,10,备注1\nD002,13800000002,普通,20,备注2\nD003,13800000003,金卡,30,备注3\nD004,13800000004,银卡,40,备注4\nD005,1380000,普通,50,备注5\nD006,,金卡,60,备注6\nD007,13800000007,银卡,70,备注7\nD008,13800000008,普通,-5,备注8\nD009,13800000009,钻石,90,备注9\nD010,13800000010,银卡,100,备注10\nD011,13800000011,普通,110,备注11\nD012,13800000012,金卡,120,备注12\nD013,13800000013,银卡,130,备注13\nD014,13800000014,普通,140,备注14\nD015,13800000015,金卡,150,备注15\nD016,13800000016,银卡,160,备注16\nD017,13800000017,普通,170,备注17\nD018,13800000018,金卡,180,备注18\nD019,13800000019,银卡,190,备注19\nD020,13800000020,普通,200,备注20\nD021,13800000021,金卡,210,备注21\nD022,13800000022,银卡,220,备注22\nD023,13800000023,普通,230,备注23\nD024,13800000024,金卡,240,备注24\nD025,13800000025,银卡,250,备注25\nD026,13800000026,普通,260,备注26\nD027,13800000027,金卡,270,备注27\nD028,13800000028,银卡,280,备注28\nD029,13800000029,普通,290,备注29\nD001,13800000030,金卡,300,备注30\n"
D = import_csv("会员名单", CSV, {"单号": "id", "金额": "money"})
suggest = lambda tok=A, d=D: call("GET", f"/ledger/datasets/{d}/validation/suggest", tok)

s, r = suggest()
sug = r["data"]["suggestions"] if s == 200 else []
got = [(x["rule"]["column"], x["rule"]["kind"], x["violations"]) for x in sug]
check("按数据推荐：编号必填且不能重复、手机号必填且是手机号格式、等级只能是常见的几个值、金额不小于 0", got == [("单号", "required", 0), ("单号", "unique", 2), ("手机号", "required", 1), ("手机号", "format", 1), ("等级", "allowed", 1), ("金额", "range", 1)], got)
by = {(x["rule"]["column"], x["rule"]["kind"]): x for x in sug}
check("取值列表不含只出现一次的写法，原因里点出来", by.get(("等级", "allowed"), {}).get("rule", {}).get("values") == ["普通", "银卡", "金卡"] and "「钻石」" in by[("等级", "allowed")]["reason"], by.get(("等级", "allowed")))
check("每条都有原因和样例", "重复" in by[("单号", "unique")]["reason"] and by[("单号", "unique")]["samples"] == ["D001"] and "手机号格式" in by[("手机号", "format")]["reason"] and by[("金额", "range")]["rule"]["min"] == 0 and "负数" in by[("金额", "range")]["reason"], sug)
check("推荐不保存", call("GET", f"/ledger/datasets/{D}/validation", A)[1]["data"]["rules"] == [], "")
call("PUT", f"/ledger/datasets/{D}/validation", A, body={"rules": [{"column": "单号", "kind": "unique"}]})
s, r = suggest()
check("已有的规则不再推荐", [(x["rule"]["column"], x["rule"]["kind"]) for x in r["data"]["suggestions"]] == [("单号", "required"), ("手机号", "required"), ("手机号", "format"), ("等级", "allowed"), ("金额", "range")], r["data"]["suggestions"])
s, r = suggest(Bo)
check("其他登录用户也能看推荐（只读）", s == 200, (s, r))
small = import_csv("三行", "名称,金额\na,1\nb,2\nc,3\n", {"金额": "money"})
s, r = suggest(d=small)
check("数据太少：不推荐", s == 200 and r["data"]["suggestions"] == [], r)

# ── AI 建议 ──
wbj = gzip.compress(json.dumps({"sheetOrder": ["s1"], "sheets": {"s1": {"id": "s1", "name": "S", "cellData": {}}}}).encode())
def thread(tok):
    s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("校验建议"), tok, raw=wbj)
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


ask("propose_validation", {"dataset": "会员名单"})
tools = [t["function"]["name"] for t in seen[0]["tools"]]
check("模型工具列表含 propose_validation", "propose_validation" in tools, tools)
acts = actions("validation")
check("不给规则：按推荐生成一条 validation 建议", len(acts) == 1, acts)
pl = acts[0]["payload"]
check("建议里是已有规则 + 推荐的 5 条，记下原来的规则", len(pl["rules"]) == 6 and pl["rules"][0] == {"column": "单号", "kind": "unique"} and pl["previous"] == [{"column": "单号", "kind": "unique"}], pl["rules"])
check("预览：每条规则不合格的行数、样例、原因", pl["preview"]["header"] == ["规则", "不合格的行", "样例", "原因"] and pl["preview"]["rows"][2][:3] == ["「手机号」是手机号格式", "1", "「1380000」"] and "新增 5 条规则" in pl["preview"]["note"], pl["preview"])
check("回给模型的内容说明等待用户确认", "等待用户确认" in last_tool() and "钻石" in last_tool(), last_tool()[:400])
check("用户确认之前不保存", call("GET", f"/ledger/datasets/{D}/validation", A)[1]["data"]["rules"] == [{"column": "单号", "kind": "unique"}], "")

ask("propose_validation", {"dataset": "会员名单", "rules": [{"column": "金额", "kind": "range", "min": 0, "max": 250}], "replace": True})
pl = actions("validation")[-1]["payload"]
check("按给的规则、替换已有规则", pl["rules"] == [{"column": "金额", "kind": "range", "min": 0, "max": 250}] and pl["preview"]["rows"][0][1] == "6" and "替换已有规则" in pl["preview"]["note"], pl)
n = len(actions("validation"))
ask("propose_validation", {"dataset": "会员名单", "rules": [{"column": "单号", "kind": "unique"}]})
check("已经有的规则：不出建议", "已经有这些规则" in last_tool() and len(actions("validation")) == n, last_tool())
ask("propose_validation", {"dataset": "会员名单", "rules": [{"column": "等级", "kind": "range", "min": 1}]})
check("规则写错：报错让模型改", last_tool().startswith("错误") and "取值范围只能用于" in last_tool() and len(actions("validation")) == n, last_tool())
ask("propose_validation", {"dataset": "三行"})
check("没有可推荐的：说明", "没有可推荐" in last_tool(), last_tool())

call("PUT", f"/ledger/data-scope/datasets/{D}", R, body={"scopeColumn": "等级", "open": False})
call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["金卡"]})
tb = thread(Bo)
ask("propose_validation", {"dataset": "会员名单"}, tok=Bo, t=tb)
check("受限用户不能提校验规则建议", "数据范围受限" in last_tool() and not actions("validation", Bo, tb), last_tool())
call("DELETE", f"/ledger/data-scope/rules/{ids['Bob']}", R)
sysmsg = seen[0]["messages"][0]["content"]
check("系统提示说明何时用 propose_validation", "propose_validation" in sysmsg, sysmsg[-300:])

srv.shutdown()
finish()
