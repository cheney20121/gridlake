# env: LEDGER_INBOX_ROOTS=@TMP@/inbox LEDGER_URL_IMPORT_ALLOW_LOCAL=1
"""外部数据集：自动检查文件变化（有变化才重新读取）、修改读取方式、dataset.refreshed 事件

先放 9 月的文件；设成每 1 分钟自动检查，放进 10 月的文件后不用手动刷新，一分钟左右数据集自动变成 1500 行，
并发出 dataset.refreshed（trigger = auto，没有操作人）。手动刷新没有变化时不重建、不发事件。
"""
import http.server
import shutil
import threading

from lib import *  # noqa: F403

FIX = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures", "external")
INBOX = os.path.join(os.environ["LEDGER_E2E_TMP"], "inbox")
# 自己的目录：test_53 在 inbox/订单 下留着文件
DIR = os.path.join(INBOX, "自动刷新")
shutil.rmtree(DIR, ignore_errors=True)
os.makedirs(DIR)
shutil.copy(os.path.join(FIX, "orders-2026-09.parquet"), DIR)

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
refreshed = lambda: [g for g in GOT if g["event"] == "dataset.refreshed"]

for t in (A, R):
    call("GET", "/ledger/me", t)
s, r = call("POST", "/ledger/webhooks", R, body={"name": "管道", "url": f"http://127.0.0.1:{HOOK_PORT}/h", "events": ["dataset.refreshed"]})
assert s == 200, (s, r)

src = {"path": "自动刷新", "pattern": "orders-*.parquet"}
s, r = call("POST", "/ledger/datasets/external", R, body={"name": "外部订单", **src})
assert s == 200, (s, r)
D = r["data"]
ext = lambda: next(d for d in call("GET", "/ledger/datasets", R)[1]["data"] if d["id"] == D)["external"]
rows = lambda: next(d for d in call("GET", "/ledger/datasets", R)[1]["data"] if d["id"] == D)["rowCount"]
check("新建：默认只手动刷新", ext()["refreshMinutes"] is None and rows() == 1000, ext())

# ── 手动刷新、没有变化 ──
s, r = call("POST", f"/ledger/datasets/{D}/external/refresh", R)
check("手动刷新：没有变化（changed = false）", s == 200 and r["data"]["changed"] is False and r["data"]["rows"] == 1000, (s, r))
time.sleep(1)
check("没有变化不发事件", not refreshed(), GOT)

# ── 修改：权限与校验 ──
s, r = call("PUT", f"/ledger/datasets/{D}/external", A, body={**src, "refreshMinutes": 1})
check("普通用户不能修改外部数据集", s == 403, (s, r))
s, r = call("PUT", f"/ledger/datasets/{D}/external", R, body={**src, "refreshMinutes": 5000})
check("间隔超出范围：拒绝", s == 400 and "1440" in msg((s, r)), (s, r))
s, r = call("PUT", f"/ledger/datasets/{D}/external", R, body={**src, "refreshMinutes": 1})
check("设为每 1 分钟自动检查（读取方式没变，不重新读取）", s == 200 and r["data"]["changed"] is False and ext()["refreshMinutes"] == 1, (s, r))

# ── 放进新文件，等自动检查 ──
shutil.copy(os.path.join(FIX, "orders-2026-10.parquet"), DIR)
end = time.time() + 120
while time.time() < end and rows() != 1500:
    time.sleep(2)
check("约一分钟后自动刷新成 1500 行（不用手动刷新）", rows() == 1500, ext())
e = ext()
check("记下检查时间、没有错误、文件数 2", e["checkedAt"] and e["checkError"] is None and e["fileCount"] == 2, e)
end = time.time() + 10
while time.time() < end and not refreshed():
    time.sleep(0.2)
ev = refreshed()[0] if refreshed() else {}
check("发出 dataset.refreshed（trigger = auto，没有操作人）", ev.get("data", {}).get("trigger") == "auto" and ev["data"]["addedFiles"] == 1 and ev["data"]["rows"] == 1500 and ev["actor"] is None, ev)
s, r = call("POST", f"/ledger/datasets/{D}/aggregate", R, body={"calls": [{"func": "count", "column": "单号", "criteria": []}]})
check("查询看到新文件的数据", s == 200 and r["data"]["values"] == [1500], (s, r))

# ── 修改读取方式：只读 10 月 ──
s, r = call("PUT", f"/ledger/datasets/{D}/external", R, body={"path": "自动刷新", "pattern": "orders-2026-10.parquet", "refreshMinutes": 1})
check("修改文件名规则：按新规则重新读取", s == 200 and r["data"]["changed"] and r["data"]["rows"] == 500 and r["data"]["removedFiles"] == 1 and rows() == 500, (s, r))
end = time.time() + 10
while time.time() < end and len(refreshed()) < 2:
    time.sleep(0.2)
check("修改也发事件（trigger = edit，带操作人）", len(refreshed()) >= 2 and refreshed()[1]["data"]["trigger"] == "edit" and refreshed()[1]["actor"]["name"] == "Root", refreshed())

# ── 目录被移走：自动检查失败，原因记下来，数据集照旧 ──
shutil.rmtree(DIR)
end = time.time() + 120
while time.time() < end and not ext()["checkError"]:
    time.sleep(2)
check("目录不见了：记下检查失败的原因", ext()["checkError"] and "自动刷新" in ext()["checkError"] and ext()["fileCount"] == 1, ext())
s, r = call("PUT", f"/ledger/datasets/{D}/external", R, body={"path": "自动刷新", "pattern": "orders-2026-10.parquet", "refreshMinutes": None})
check("改回只手动刷新", s == 200 and ext()["refreshMinutes"] is None, (s, r))
time.sleep(2)  # 审计每 2 秒批量写一次
s, r = call("GET", "/ledger/audit?size=100", R)
check("修改进审计", "修改外部数据集" in {x["action"] for x in r["data"]["items"]}, None)
finish()
