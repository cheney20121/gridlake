"""AI 另存为数据集建议（save_query_as_dataset）

用脚本化的模拟模型驱动 AI 助手：模型调用 save_query_as_dataset，服务端先执行检查（只读、语法、结果列名合规、总行数、前几行、
名称不重名），生成一张 dataset_save 建议；**用户确认之前不创建任何数据集**。创建与撤销发生在浏览器里（见前端界面测试）。
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
s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("另存数据集测试"), A, raw=wbj); wb = r["data"]["id"]
s, r = call("POST", "/ledger/ai/threads", A, body={"workbookId": wb}); check("创建会话", s == 200, (s, r)); tid = r["data"]["id"]


def ask(text, args):
    script.extend([{"tool_calls": [("save_query_as_dataset", args)]}, {"content": "好的。"}])
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



def saves():
    return [a for a in detail()["actions"] if a["kind"] == "dataset_save"]


def dataset_names():
    s, r = call("GET", "/ledger/datasets", A)
    return [d["name"] for d in r["data"]]


SQL = "SELECT 店铺, SUM(金额) AS 销售额, COUNT(*) AS 订单数 FROM 订单 GROUP BY 店铺 ORDER BY 店铺"
before = dataset_names()

ask("把各店铺的销售额存成数据集", {"sql": SQL, "name": "店铺汇总", "purpose": "各店铺销售额与订单数"})
names = [t["function"]["name"] for t in seen[0]["tools"]]
check("模型工具列表含 save_query_as_dataset", "save_query_as_dataset" in names, names)
acts = saves()
check("生成一条 dataset_save 建议", len(acts) == 1, [a["kind"] for a in detail()["actions"]])
pl = acts[0]["payload"]
check("建议带名称、SQL、用途、总行数", pl["name"] == "店铺汇总" and pl["sql"] == SQL and pl["purpose"] == "各店铺销售额与订单数" and pl["rows"] == 2, pl)
check("预览：列与前几行", pl["preview"]["header"] == ["店铺", "销售额", "订单数"] and [r[0] for r in pl["preview"]["rows"]] == ["A", "B"] and pl["preview"]["rows"][0][1] in ("40", "40.00"), pl["preview"])
check("预览说明行列数、用到的数据集、不自动刷新", "共 2 行 × 3 列" in pl["preview"]["note"] and "用到数据集：订单" in pl["preview"]["note"] and "不会自动刷新" in pl["preview"]["note"], pl["preview"]["note"])
check("标题", acts[0]["title"] == "另存为数据集「店铺汇总」（2 行 × 3 列）", acts[0]["title"])
check("用户确认之前没有创建任何数据集", dataset_names() == before, (before, dataset_names()))


def bad(label, args, expect):
    n = len(saves())
    ask(label, args)
    msg_ = last_tool_content()
    check(label, expect in msg_ and len(saves()) == n and dataset_names() == before, msg_[:240])


bad("名称与已有数据集重名", {"sql": SQL, "name": "订单"}, "已有名为「订单」的数据集")
bad("名称为空", {"sql": SQL, "name": "  "}, "起个名字")
bad("名称太长", {"sql": SQL, "name": "长" * 61}, "最长 60 个字")
bad("名称含双引号", {"sql": SQL, "name": '汇"总'}, "双引号")
bad("只能是只读查询", {"sql": "DELETE FROM 订单", "name": "危险"}, "SELECT")
bad("引用不存在的数据集", {"sql": "SELECT * FROM 没有这张表", "name": "无效"}, "没有这张表")
bad("结果列名重复", {"sql": "SELECT 店铺, 店铺 FROM 订单", "name": "重复列"}, "重复")
bad("结果里有保留列名", {"sql": "SELECT 店铺, 1 AS rowid FROM 订单", "name": "保留列"}, "保留列名")
bad("结果是 0 行", {"sql": "SELECT * FROM 订单 WHERE 金额 > 999999", "name": "空结果"}, "0 行")

# 前端「应用」与「撤销」用到的接口序列：保存查询并另存为数据集 → 查引用 → 删查询（数据集保留）→ 删数据集（进回收站）
import time
s, r = call("POST", "/ledger/sql/queries", A, body={"name": "店铺汇总", "sql": SQL, "description": "AI", "saveAsDataset": "店铺汇总", "schedule": {"kind": "manual"}, "operator": "Alice"})
check("按建议另存为数据集", s == 200 and r["data"]["datasetId"] and r["data"]["lastRows"] == 2, (s, r))
qid, dsid = r["data"]["id"], r["data"]["datasetId"]
check("另存后数据集出现", "店铺汇总" in dataset_names())
t0 = time.time()
s, r = call("GET", f"/ledger/datasets/{dsid}/usage", A)
check("查引用能返回，且只被自己的查询引用", s == 200 and [q["id"] for q in r["data"]["sqlQueries"]] == [qid] and not r["data"]["workbooks"] and r["data"]["writebacks"] == 0, (s, r))
print(f"  引用查询耗时 {time.time() - t0:.2f}s")
s, r = call("GET", "/ledger/datasets", A)
mine = [d for d in r["data"] if d["name"] == "店铺汇总"][0]
check("另存出来的数据集归创建人（否则连自己也删不掉）", mine["ownerId"] and mine["ownerName"], mine)
s, r = call("DELETE", f"/ledger/datasets/{dsid}?operator=Bob", Bo)
check("别的用户不能删它", s == 403, (s, r))
s, r = call("DELETE", f"/ledger/sql/queries/{qid}", A)
check("删除查询（数据集保留）", s == 200 and "店铺汇总" in dataset_names(), (s, r))
s, r = call("DELETE", f"/ledger/datasets/{dsid}?operator=Alice", A)
check("删除数据集（移入回收站）", s == 200 and "店铺汇总" not in dataset_names(), (s, r))
s, r = call("GET", "/ledger/datasets/trash", A)
check("数据集在回收站里，可恢复", "店铺汇总" in json.dumps(r["data"], ensure_ascii=False), r)

sysmsg = seen[0]["messages"][0]["content"]
check("系统提示说明何时另存为数据集", "save_query_as_dataset" in sysmsg, sysmsg[-600:])

srv.shutdown()
finish()
