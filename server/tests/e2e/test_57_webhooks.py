# env: LEDGER_URL_IMPORT_ALLOW_LOCAL=1
"""事件 Webhook：只有管理员能管理；导入 / 填报 / 删除数据集时按订阅投递，签名可验证；
失败排队重试、手动重发（同一个投递 id）；没订阅的事件不发；重置密钥；停用后不发；测试 ping。

本机起一个接收器，按路径区分两个 Webhook；/h1 可以切换成返回 500。
"""
import hashlib
import hmac
import http.server
import threading

from lib import *  # noqa: F403

HOOK_PORT = int(os.environ.get("LEDGER_E2E_WEBHOOK_PORT", "19005"))
GOT = []          # (path, headers, body 文本)
FAIL = {"/h1": False}


class Hook(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n).decode()
        GOT.append((self.path.split("?")[0], {k.lower(): v for k, v in self.headers.items()}, body))
        code = 500 if FAIL.get(self.path.split("?")[0]) else 200
        self.send_response(code)
        self.send_header("Content-Type", "text/plain")
        self.end_headers()
        self.wfile.write(b"boom" if code == 500 else b"ok")

    def log_message(self, *a):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", HOOK_PORT), Hook)
threading.Thread(target=server.serve_forever, daemon=True).start()
URL = f"http://127.0.0.1:{HOOK_PORT}"


def verified(headers, body, secret):
    parts = dict(p.split("=", 1) for p in headers.get("x-gridlake-signature", "").split(","))
    want = hmac.new(secret.encode(), f"{parts.get('t')}.{body}".encode(), hashlib.sha256).hexdigest()
    return hmac.compare_digest(want, parts.get("v1", "")) and abs(time.time() - int(parts.get("t", 0))) < 300


def wait_for(pred, secs=10):
    end = time.time() + secs
    while time.time() < end:
        hit = [g for g in GOT if pred(g)]
        if hit:
            return hit
        time.sleep(0.1)
    return []


def event_of(path, name):
    return lambda g: g[0] == path and json.loads(g[2]).get("event") == name


for t in (A, Bo, R): call("GET", "/ledger/me", t)

# ── 管理：只有管理员 ──
s, r = call("GET", "/ledger/webhooks", A)
check("普通用户看不到 Webhook", s == 403, (s, r))
s, r = call("GET", "/ledger/webhooks", R)
check("列出可订阅的事件", s == 200 and {"dataset.created", "connector.synced", "alert.triggered", "approval.decided"} <= {e["name"] for e in r["data"]["events"]}, (s, r))
EVENTS = ["dataset.created", "dataset.imported", "dataset.deleted", "writeback.submitted"]
s, r = call("POST", "/ledger/webhooks", A, body={"name": "管道", "url": f"{URL}/h1", "events": EVENTS})
check("普通用户不能新建", s == 403, (s, r))
s, r = call("POST", "/ledger/webhooks", R, body={"name": "管道", "url": f"{URL}/h1", "events": ["dataset.exploded"]})
check("不认识的事件：拒绝", s == 400 and "dataset.exploded" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/webhooks", R, body={"name": "管道", "url": "ftp://example.com/x", "events": EVENTS})
check("只收 http / https", s == 400, (s, r))
s, r = call("POST", "/ledger/webhooks", R, body={"name": "管道", "url": f"{URL}/h1?token=s3cret", "events": EVENTS})
check("新建：返回签名密钥", s == 200 and r["data"]["secret"].startswith("whsec_"), (s, r))
H1, SECRET = r["data"]["id"], r["data"]["secret"]
s, r = call("POST", "/ledger/webhooks", R, body={"name": "审批机器人", "url": f"{URL}/h2", "events": ["approval.decided"]})
H2 = r["data"]["id"]
s, r = call("POST", "/ledger/webhooks", R, body={"name": "管道", "url": f"{URL}/h3", "events": ["*"]})
check("名称不能重复", s == 400, (s, r))
hooks = {h["id"]: h for h in call("GET", "/ledger/webhooks", R)[1]["data"]["hooks"]}
check("列表：地址里的查询参数隐藏、不返回密钥", hooks[H1]["url"] == f"{URL}/h1?…" and "secret" not in hooks[H1] and SECRET not in json.dumps(hooks), hooks[H1])

# ── 测试 ping ──
s, r = call("POST", f"/ledger/webhooks/{H1}/test", R)
check("测试：发送成功", s == 200 and r["data"]["status"] == "success" and r["data"]["httpStatus"] == 200 and r["data"]["event"] == "ping", (s, r))
g = wait_for(event_of("/h1", "ping"))
check("接收方收到 ping，带事件名、投递 id，签名可验证", g and g[0][1]["x-gridlake-event"] == "ping" and g[0][1]["x-gridlake-delivery"] == json.loads(g[0][2])["id"] and verified(g[0][1], g[0][2], SECRET), g)

# ── 导入新建 → dataset.created ──
boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()

def import_csv(csv, mode, dataset_id=None, name="订单"):
    s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("订单.csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
    assert s == 200, (s, r)
    p = r["data"]
    s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": mode, "datasetId": dataset_id, "name": name, "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
    assert s == 200, (s, r)
    return r["data"]["datasetId"]

D = import_csv("店铺,金额\nA,10\nB,20\n", "create")
g = wait_for(event_of("/h1", "dataset.created"))
body = json.loads(g[0][2]) if g else {}
check("导入新建：收到 dataset.created（数据集、行数、操作人）", body.get("data", {}).get("datasetId") == D and body["data"]["rowsRead"] == 2 and body["data"]["source"] == "import" and body["actor"]["name"] == "Alice", body)
check("签名可验证，篡改后验证不过", g and verified(g[0][1], g[0][2], SECRET) and not verified(g[0][1], g[0][2].replace("Alice", "Mallory"), SECRET), g)

import_csv("店铺,金额\nC,30\n", "append", D)
g = wait_for(event_of("/h1", "dataset.imported"))
check("追加导入：收到 dataset.imported", g and json.loads(g[0][2])["data"]["mode"] == "append" and json.loads(g[0][2])["data"]["inserted"] == 1, g)

# ── 填报 → dataset.created（input）+ writeback.submitted ──
s, r = call("POST", "/ledger/datasets/input", A, body={"name": "日报", "columns": [{"title": "日期", "kind": "text"}, {"title": "件数", "kind": "integer"}], "primaryKey": ["日期"]})
I = r["data"]
s, r = call("POST", f"/ledger/datasets/{I}/writeback", A, body={"mode": "upsert", "columns": ["日期", "件数"], "rows": [["10-01", 3], ["10-02", 5]]})
assert s == 200, (s, r)
g = wait_for(event_of("/h1", "writeback.submitted"))
check("填报提交：收到 writeback.submitted", g and json.loads(g[0][2])["data"]["datasetId"] == I and json.loads(g[0][2])["data"]["inserted"] == 2, g)
check("新建填报数据集：收到 dataset.created（source=input）", any(json.loads(x[2])["data"].get("source") == "input" for x in wait_for(event_of("/h1", "dataset.created"))), GOT)
check("没订阅的事件不发：/h2 只订了审批", not [x for x in GOT if x[0] == "/h2"], GOT)

# ── 审批通过 → /h2 的 approval.decided ──
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}
wbj = gzip.compress(json.dumps({"sheetOrder": ["s1"], "sheets": {"s1": {"id": "s1", "name": "S", "cellData": {}}}}).encode())
s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("月报"), A, raw=wbj)
s, r = call("POST", "/ledger/approvals", A, body={"workbookId": r["data"]["id"], "approverId": ids["Bob"], "comment": "请审"})
assert s == 200, (s, r)
APPROVAL = r["data"]["id"]
time.sleep(0.5)
check("发起审批：/h2 没订 approval.created，不发", not [x for x in GOT if x[0] == "/h2"], GOT)
s, r = call("POST", f"/ledger/approvals/{APPROVAL}/decide", Bo, body={"approve": True, "comment": "同意"})
g = wait_for(event_of("/h2", "approval.decided"))
d = json.loads(g[0][2]) if g else {}
check("审批通过：/h2 收到 approval.decided", d.get("data", {}).get("approvalId") == APPROVAL and d["data"]["status"] == "approved" and d["data"]["workbookName"] == "月报" and d["actor"]["name"] == "Bob" and d["data"]["decisionComment"] == "同意", d)

# ── SQL 重算 → sql.refreshed（订阅全部事件的 Webhook）──
s, r = call("POST", "/ledger/webhooks", R, body={"name": "全部", "url": f"{URL}/h3", "events": ["*", "dataset.created"]})
check("订阅全部事件（* 与具体事件同时选时只留 *）", s == 200 and next(h for h in call("GET", "/ledger/webhooks", R)[1]["data"]["hooks"] if h["name"] == "全部")["events"] == ["*"], (s, r))
s, r = call("POST", "/ledger/sql/queries", A, body={"name": "店铺汇总", "sql": "SELECT 店铺, sum(金额) AS 金额 FROM 订单 GROUP BY 1", "schedule": {"kind": "manual"}})
assert s == 200, (s, r)
s, r = call("POST", f"/ledger/sql/queries/{r['data']['id']}/refresh", A, body={"datasetName": "店铺汇总"})
assert s == 200, (s, r)
g = wait_for(event_of("/h3", "sql.refreshed"))
d = json.loads(g[0][2])["data"] if g else {}
check("SQL 重算：收到 sql.refreshed（行数、数据集）", d.get("status") == "success" and d["rows"] == 3 and d["datasetName"] == "店铺汇总", d)

# ── 失败 → 排队重试 → 手动重发 ──
FAIL["/h1"] = True
s, r = call("DELETE", f"/ledger/datasets/{I}", A)
assert s == 200, (s, r)
assert wait_for(event_of("/h1", "dataset.deleted")), GOT
time.sleep(0.5)
s, r = call("GET", f"/ledger/webhooks/{H1}/deliveries", R)
dl = next(d for d in r["data"] if d["event"] == "dataset.deleted")
check("对方返回 500：记下失败、排队重试", dl["status"] == "pending" and dl["attempts"] == 1 and dl["httpStatus"] == 500 and dl["nextAt"] and dl["response"] == "boom" and "500" in dl["error"], dl)
check("列表里有排队数和最近一次失败", next(h for h in call("GET", "/ledger/webhooks", R)[1]["data"]["hooks"] if h["id"] == H1)["pending"] == 1, None)
FAIL["/h1"] = False
s, r = call("POST", f"/ledger/webhooks/deliveries/{dl['id']}/redeliver", R)
check("手动重发", s == 200, (s, r))
end = time.time() + 10
while time.time() < end and len([x for x in GOT if x[1].get("x-gridlake-delivery") == dl["id"]]) < 2:
    time.sleep(0.1)
deliveries = [x for x in GOT if x[1].get("x-gridlake-delivery") == dl["id"]]
check("重发用同一个投递 id 和请求体（接收方可去重）", len(deliveries) >= 2 and deliveries[0][2] == deliveries[-1][2], deliveries)
time.sleep(0.5)
dl2 = next(d for d in call("GET", f"/ledger/webhooks/{H1}/deliveries", R)[1]["data"] if d["id"] == dl["id"])
check("重发成功后状态变成 success", dl2["status"] == "success" and dl2["nextAt"] is None, dl2)

# ── 重置密钥 ──
s, r = call("POST", f"/ledger/webhooks/{H1}/secret", R)
NEW = r["data"]
check("重置密钥：新密钥不同", s == 200 and NEW.startswith("whsec_") and NEW != SECRET, (s, r))
call("POST", f"/ledger/webhooks/{H1}/test", R)
last = [x for x in GOT if x[0] == "/h1"][-1]
check("之后用新密钥签名，旧密钥验证不过", verified(last[1], last[2], NEW) and not verified(last[1], last[2], SECRET), last[1])

# ── 停用后不发 ──
s, r = call("PUT", f"/ledger/webhooks/{H1}", R, body={"name": "管道", "events": EVENTS, "enabled": False})
check("停用（地址不填表示不变）", s == 200 and next(h for h in call("GET", "/ledger/webhooks", R)[1]["data"]["hooks"] if h["id"] == H1)["url"] == f"{URL}/h1?…", (s, r))
n_before = len(GOT)
import_csv("店铺,金额\nD,40\n", "append", D)
time.sleep(1.5)
check("停用的 Webhook 不再收到事件", not [x for x in GOT[n_before:] if x[0] == "/h1"], GOT[n_before:])
s, r = call("POST", f"/ledger/webhooks/deliveries/{dl['id']}/redeliver", R)
check("停用时不能重发", s == 400 and "停用" in msg((s, r)), (s, r))

# ── 删除 ──
s, r = call("DELETE", f"/ledger/webhooks/{H1}", A)
check("普通用户不能删除", s == 403, (s, r))
s, r = call("DELETE", f"/ledger/webhooks/{H1}", R)
check("删除（连同发送记录）", s == 200 and H1 not in {h["id"] for h in call("GET", "/ledger/webhooks", R)[1]["data"]["hooks"]}, (s, r))
s, r = call("GET", f"/ledger/webhooks/{H1}/deliveries", R)
check("删除后发送记录也没了", s == 400, (s, r))
time.sleep(3)  # 审计每 2 秒批量写一次
s, r = call("GET", "/ledger/audit?size=100", R)
acts = {x["action"] for x in r["data"]["items"]} if s == 200 else set()
check("管理操作进审计", {"新建 Webhook", "删除 Webhook", "重置 Webhook 签名密钥"} <= acts, acts)
finish()
