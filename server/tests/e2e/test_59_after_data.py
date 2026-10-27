# env: LEDGER_URL_IMPORT_ALLOW_LOCAL=1
"""「数据集更新后」触发：订单导入新数据 → 设为「订单更新后」的 SQL 查询自动重算 → 重算写入的汇总数据集
再触发设为「汇总更新后」的告警。SQL 查询写入的数据集不会再触发别的 SQL 查询（不会互相触发）。
填报提交不触发（太频繁）。

本机起一个接收器当钉钉渠道，统计告警消息。
"""
import http.server
import threading

from lib import *  # noqa: F403

HOOK_PORT = int(os.environ.get("LEDGER_E2E_HOOK_PORT2", "19007"))
HOOKS = []


class Hook(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        HOOKS.append(json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"null"))
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"errcode":0}')

    def log_message(self, *a):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", HOOK_PORT), Hook)
threading.Thread(target=server.serve_forever, daemon=True).start()

for t in (A, R):
    call("GET", "/ledger/me", t)

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()

def import_csv(csv, mode, dataset_id=None):
    s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("订单.csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
    assert s == 200, (s, r)
    p = r["data"]
    s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": mode, "datasetId": dataset_id, "name": "订单", "columns": [{"source": c["source"], "title": c["title"], "kind": "money" if c["title"] == "金额" else c["kind"]} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
    assert s == 200, (s, r)
    return r["data"]["datasetId"]

D = import_csv("店铺,金额\nA,100\nB,50\n", "create")

def wait(pred, secs=20):
    end = time.time() + secs
    while time.time() < end:
        if pred():
            return True
        time.sleep(0.3)
    return False

# ── SQL 查询：订单更新后重算，写入「店铺汇总」──
q = lambda **kw: {"name": "店铺汇总", "sql": "SELECT 店铺, sum(金额) AS 金额 FROM 订单 GROUP BY 1 ORDER BY 1", **kw}
s, r = call("POST", "/ledger/sql/queries", A, body=q(schedule={"kind": "after_data", "datasetId": "nope"}))
check("触发用的数据集要存在", s == 400 and "数据集" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/sql/queries", A, body=q(saveAsDataset="店铺汇总", schedule={"kind": "after_data", "datasetId": D}))
check("保存：订单更新后重算", s == 200 and r["data"]["schedule"] == {"kind": "after_data", "datasetId": D} and r["data"]["datasetId"], (s, r))
Q, S = r["data"]["id"], r["data"]["datasetId"]
query = lambda: next(x for x in call("GET", "/ledger/sql/queries", A)[1]["data"] if x["id"] == Q)
total = lambda ds, shop=None: call("POST", f"/ledger/datasets/{ds}/aggregate", A, body={"calls": [{"func": "sum", "column": "金额", "criteria": [{"column": "店铺", "value": shop}] if shop else []}]})[1]["data"]["values"][0]
check("先算了一次：汇总 150", total(S) == 150, total(S))
first_run = query()["lastRunAt"]

# ── 告警：店铺汇总更新后，A 店金额超过 200 就通知 ──
s, r = call("POST", "/ledger/report-channels", R, body={"name": "运营群", "kind": "dingtalk", "target": f"http://127.0.0.1:{HOOK_PORT}/hook"})
CH = r["data"]
rule = {"name": "A 店金额偏高", "datasetId": S, "metric": {"kind": "agg", "column": "金额", "agg": "sum"}, "metricLabel": "金额", "groupBy": "店铺",
        "filters": [], "window": {"dateColumn": None, "preset": "none"}, "condition": {"op": "gt", "threshold": 200}, "guard": None,
        "schedule": {"kind": "after_data", "datasetId": S}, "channelIds": [CH], "repeat": {"kind": "until_recovered"}}
s, r = call("POST", "/ledger/alerts", A, body=rule)
check("告警：汇总更新后检查", s == 200, (s, r))
AL = r["data"]

# ── 追加导入订单 → SQL 重算 → 告警 ──
time.sleep(1.1)  # 让重算时间和第一次不同
import_csv("店铺,金额\nA,300\n", "append", D)
check("追加导入后，SQL 查询自动重算", wait(lambda: query()["lastRunAt"] != first_run and total(S, "A") == 400), query())
check("汇总数据集更新后，告警自动检查并通知（A 店 400 > 200）", wait(lambda: len(HOOKS) >= 1), HOOKS)
check("通知内容是这条告警", HOOKS and "A 店金额偏高" in json.dumps(HOOKS[0], ensure_ascii=False), HOOKS)
s, r = call("GET", f"/ledger/alerts/{AL}/runs", A)
check("告警的检查记录：触发方式是「数据集更新后」", s == 200 and r["data"] and r["data"][0]["trigger"] == "after_data", (s, r))

# ── SQL 写入的数据集不再触发别的 SQL 查询 ──
s, r = call("POST", "/ledger/sql/queries", A, body={"name": "汇总的汇总", "sql": "SELECT count(*) AS 店铺数 FROM 店铺汇总", "saveAsDataset": "店铺数", "schedule": {"kind": "after_data", "datasetId": S}})
assert s == 200, (s, r)
Q2 = r["data"]["id"]
second = lambda: next(x for x in call("GET", "/ledger/sql/queries", A)[1]["data"] if x["id"] == Q2)["lastRunAt"]
before = second()
run_before = query()["lastRunAt"]
time.sleep(1.1)
import_csv("店铺,金额\nC,1\n", "append", D)
wait(lambda: query()["lastRunAt"] != run_before)
time.sleep(2)
check("SQL 查询重算写入的数据集，不会再触发别的 SQL 查询", query()["lastRunAt"] != run_before and second() == before, (query()["lastRunAt"], second()))

# ── 填报提交不触发 ──
s, r = call("POST", "/ledger/datasets/input", A, body={"name": "日报", "columns": [{"title": "店铺", "kind": "text"}, {"title": "金额", "kind": "money"}], "primaryKey": ["店铺"]})
I = r["data"]
s, r = call("PUT", f"/ledger/sql/queries/{Q}", A, body=q(schedule={"kind": "after_data", "datasetId": I}))
check("改成「日报更新后」", s == 200, (s, r))
run_before = query()["lastRunAt"]
time.sleep(1.1)
call("POST", f"/ledger/datasets/{I}/writeback", A, body={"mode": "upsert", "columns": ["店铺", "金额"], "rows": [["A", 1]]})
time.sleep(3)
check("填报提交不触发重算（太频繁）", query()["lastRunAt"] == run_before, query())
finish()
