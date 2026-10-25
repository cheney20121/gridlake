# env: LEDGER_URL_IMPORT_ALLOW_LOCAL=1
"""数据校验规则：必填、不能重复、取值范围、格式（手机号等 / 自定义正则）、只能是某几个值；
保存后立即检查（每条规则的不合格行数、样例、至少违反一条的行数）；数据表可以只看不合格的行（能透视）；
导入后自动重新检查并发 dataset.validation_failed 事件；只有所有者 / 管理员能改规则；数据范围受限的用户检查自己能看到的行。
"""
import http.server
import threading

from lib import *  # noqa: F403

HOOK_PORT = int(os.environ.get("LEDGER_E2E_WEBHOOK_PORT", "19006"))
GOT = []


class Hook(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        GOT.append(json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0)))))
        self.send_response(200)
        self.end_headers()

    def log_message(self, *a):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", HOOK_PORT), Hook)
threading.Thread(target=server.serve_forever, daemon=True).start()
failed_events = lambda: [g for g in GOT if g["event"] == "dataset.validation_failed"]

for t in (A, Bo, R):
    call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}
s, r = call("POST", "/ledger/webhooks", R, body={"name": "校验", "url": f"http://127.0.0.1:{HOOK_PORT}/h", "events": ["dataset.validation_failed"]})
check("订阅 dataset.validation_failed", s == 200, (s, r))

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
KINDS = {"编号": "id", "金额": "money", "日期": "datetime"}
def import_csv(csv, mode, dataset_id=None):
    s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("会员.csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
    p = r["data"]
    s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": mode, "datasetId": dataset_id, "name": "校验会员", "columns": [{"source": c["source"], "title": c["title"], "kind": KINDS.get(c["title"], "text")} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
    assert s == 200, (s, r)
    return r["data"]["datasetId"]

D = import_csv("编号,手机号,金额,日期,等级\nA1,13800000000,10,2026-09-01,金卡\nA2,1380000,-5,2026-09-02,银卡\nA2,,20000,2026-09-03,钻石\nA3,13900000000,30,2026-09-04,金卡\n", "create")
info = lambda: next(d for d in call("GET", "/ledger/datasets", A)[1]["data"] if d["id"] == D)
spec = lambda **k: {"filters": [], "keyword": None, "sort": [], "rule": None, **k}

s, r = call("GET", f"/ledger/datasets/{D}/validation", A)
check("还没有规则", s == 200 and r["data"]["rules"] == [] and r["data"]["report"] is None and "validation" not in info(), (s, r))

RULES = [
    {"column": "手机号", "kind": "required"},
    {"column": "编号", "kind": "unique"},
    {"column": "金额", "kind": "range", "min": 0, "max": 10000},
    {"column": "手机号", "kind": "format", "format": "phone"},
    {"column": "等级", "kind": "allowed", "values": ["金卡", "银卡"]},
]
s, r = call("PUT", f"/ledger/datasets/{D}/validation", Bo, body={"rules": RULES})
check("不是所有者：不能设规则", s == 403, (s, r))
s, r = call("PUT", f"/ledger/datasets/{D}/validation", A, body={"rules": [{"column": "等级", "kind": "range", "min": 1}]})
check("文本列不能设范围，说明原因", s == 400 and "取值范围只能用于" in msg((s, r)), (s, r))
s, r = call("PUT", f"/ledger/datasets/{D}/validation", A, body={"rules": [{"column": "手机号", "kind": "format", "format": "regex", "pattern": "("}]})
check("正则写错：拒绝", s == 400 and "正则表达式写得不对" in msg((s, r)), (s, r))

s, r = call("PUT", f"/ledger/datasets/{D}/validation", A, body={"rules": RULES})
rep = r["data"]["report"]
check("保存后立即检查：每条规则的不合格行数", s == 200 and [x["violations"] for x in rep["results"]] == [1, 2, 2, 1, 1], (s, r))
check("说明、样例、至少违反一条的行数", rep["results"][2]["label"] == "「金额」在 0 ~ 10000 之间" and sorted(rep["results"][2]["samples"]) == ["-5.0", "20000.0"] and rep["badRows"] == 2 and rep["failed"] == 5 and r["data"]["updatedBy"], rep)
v = info()["validation"]
check("数据集列表带小结", v["rules"] == 5 and v["failed"] == 5 and v["badRows"] == 2, v)

# ── 只看不合格的行 ──
s, r = call("POST", f"/ledger/datasets/{D}/cursor", A, body=spec(violation=RULES[1]))
check("只看「编号」重复的行", s == 200 and r["data"]["total"] == 2, (s, r))
c = r["data"]["cursorId"]
s, r = call("GET", f"/ledger/datasets/{D}/rows?cursor={c}&offset=0&limit=10", A)
check("取到的都是 A2", s == 200 and {row[0] for row in r["data"]["rows"]} == {"A2"}, r)
s, r = call("POST", f"/ledger/datasets/{D}/cursor", A, body=spec(violation=RULES[2], filters=[{"op": "in", "column": "等级", "values": ["钻石"]}]))
check("和其他筛选一起用", s == 200 and r["data"]["total"] == 1, (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/pivot", A, body={"rows": [{"column": "等级"}], "values": [{"agg": "count"}], "filters": [], "violation": RULES[2]})
check("不合格的行也能透视", s == 200 and sorted(tuple(x["keys"]) for x in r["data"]["rows"] if x["kind"] == "data") == [("钻石",), ("银卡",)], (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/clean/preview", A, body={"op": "delete_rows", "spec": spec(violation=RULES[2])})
check("可以把不合格的行交给数据清洗删除（先预览）", s == 200 and r["data"]["affected"] == 2 and r["data"]["deletes"], (s, r))

# ── 导入后自动重新检查 ──
before = len(failed_events())
import_csv("编号,手机号,金额,日期,等级\nA4,abc,50,2026-09-05,铜卡\n", "append", D)
end = time.time() + 20
while time.time() < end and (info()["validation"]["badRows"] != 3 or len(failed_events()) == before):
    time.sleep(0.3)
check("导入后自动重新检查：不合格的行数更新", info()["validation"]["badRows"] == 3, info()["validation"])
ev = failed_events()[before:] if len(failed_events()) > before else []
check("发 dataset.validation_failed 事件，带数据集和每条规则的不合格行数", ev and ev[-1]["data"]["datasetId"] == D and ev[-1]["data"]["badRows"] == 3 and {"rule": "「等级」只能是 金卡 / 银卡", "violations": 2} in ev[-1]["data"]["failed"], ev[-1:] or GOT[-2:])

# ── 数据范围 ──
call("PUT", f"/ledger/data-scope/datasets/{D}", R, body={"scopeColumn": "等级", "open": False})
call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["金卡"]})
s, r = call("POST", f"/ledger/datasets/{D}/validation/check", Bo)
check("受限用户的检查只看自己能看到的行", s == 200 and r["data"]["rows"] == 2 and r["data"]["badRows"] == 0, (s, r))
check("受限用户的检查结果不覆盖整体结果", info()["validation"]["badRows"] == 3, info()["validation"])
call("DELETE", f"/ledger/data-scope/rules/{ids['Bob']}", R)

s, r = call("PUT", f"/ledger/datasets/{D}/validation", A, body={"rules": []})
check("清空规则：列表里不再有小结", s == 200 and "validation" not in info(), info())
s, r = call("POST", f"/ledger/datasets/{D}/validation/check", A)
check("没有规则时检查：说明", s == 400 and "还没有设置校验规则" in msg((s, r)), (s, r))
time.sleep(2.5)
s, r = call("GET", "/ledger/audit?size=100", R)
check("改规则进审计", "设置数据校验规则" in {x["action"] for x in r["data"]["items"]}, None)
server.shutdown()
finish()
