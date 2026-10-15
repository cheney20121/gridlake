# env: LEDGER_URL_IMPORT_ALLOW_LOCAL=1
"""阈值告警（第 2 阶段）：较上期变化、数据停更、恢复通知

场景一（销售额环比）：近 7 天 vs 再往前 7 天。
  A 店 200 → 100（-50%）；B 店持平；C 店 100 → 0（卖着卖着没了）；D 店上期没有（不判断）；E 店上期只有 1 单（样本量不足）
场景二（数据停更）：A 店 1 天前还有订单，B 店 5 天前，C 店 10 天前。
场景三（恢复通知）：A 店补回销售额后，收到「已恢复」。
"""
import datetime
import http.server
import threading

from lib import *  # noqa: F403

HOOK_PORT = int(os.environ.get("LEDGER_E2E_HOOK_PORT", "19003"))
HOOKS = []


class Hook(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        HOOKS.append(json.loads(self.rfile.read(n) or b"null"))
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"errcode":0}')

    def log_message(self, *a):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", HOOK_PORT), Hook)
threading.Thread(target=server.serve_forever, daemon=True).start()
text_of = lambda i: json.dumps(HOOKS[i], ensure_ascii=False)

for t in (A, R): call("GET", "/ledger/me", t)
time.sleep(1)

today = datetime.date.today()
day = lambda n: (today - datetime.timedelta(days=n)).isoformat()

# ── 数据：销售（单号、店铺、日期、金额）──
s, r = call("POST", "/ledger/datasets/input", A, body={"name": "销售", "columns": [
    {"title": "单号", "kind": "text"}, {"title": "店铺", "kind": "text"}, {"title": "日期", "kind": "datetime"},
    {"title": "金额", "kind": "money"}], "primaryKey": ["单号"]})
assert s == 200, (s, r)
DS = r["data"]
COLS = ["单号", "店铺", "日期", "金额"]
rows = []
rows += [[f"A-now{i}", "A", day(1 + i), 100] for i in range(7)]      # A 本期 700
rows += [[f"A-old{i}", "A", day(8 + i), 200] for i in range(7)]      # A 上期 1400  → -50%
rows += [[f"B-now{i}", "B", day(1 + i), 100] for i in range(7)]      # B 700 → 700
rows += [[f"B-old{i}", "B", day(8 + i), 100] for i in range(7)]
rows += [[f"C-old{i}", "C", day(8 + i), 100] for i in range(7)]      # C 本期没有 → 0（-100%）
rows += [[f"D-now{i}", "D", day(1 + i), 100] for i in range(7)]      # D 上期没有
rows += [["E-old0", "E", day(9), 100]]                               # E 上期只有 1 单
s, r = call("POST", f"/ledger/datasets/{DS}/writeback", A, body={"mode": "upsert", "columns": COLS, "rows": rows})
assert s == 200 and r["data"]["inserted"] == len(rows), (s, r)

s, r = call("POST", "/ledger/report-channels", R, body={"name": "运营群", "kind": "dingtalk", "target": f"http://127.0.0.1:{HOOK_PORT}/hook"})
assert s == 200, (s, r)
CH = r["data"]

SUM = {"kind": "agg", "agg": "sum", "column": "金额"}
DROP = {"op": "lt", "threshold": -0.3, "basis": "change_pct"}
rule = lambda **kw: {
    "name": "店铺销售额下滑", "datasetId": DS, "metric": SUM, "metricLabel": "销售额", "groupBy": "店铺", "filters": [],
    "window": {"dateColumn": "日期", "preset": "last_7"}, "condition": DROP,
    "guard": {"agg": "count", "min": 5}, "schedule": {"kind": "manual"}, "channelIds": [CH], "repeat": {"kind": "until_recovered"}, **kw}

# ── 试算：较上期变化 ──
s, r = call("POST", "/ledger/alerts/preview", A, body=rule())
d = r["data"] if s == 200 else r
by = {x["key"]: x for x in d["rows"]} if s == 200 else {}
check("较上期：A 店下滑 50%、C 店掉到 0 超出条件", s == 200 and d["hits"] == 2 and by["A"]["verdict"] == "hit" and by["C"]["verdict"] == "hit", (s, r))
check("较上期：A 店本期 / 上期 / 变化都显示", by["A"]["prevText"] == "1,400.00" and by["A"]["changeText"] == "-50.0%" and by["A"]["valueText"] == "700.00", by.get("A"))
check("较上期：上期有、本期没有的分组按 0 处理（-100%）", by["C"]["valueText"] == "0.00" and by["C"]["changeText"] == "-100.0%", by.get("C"))
check("较上期：持平的 B 店正常", by["B"]["verdict"] == "clear" and by["B"]["changeText"] == "0.0%", by.get("B"))
check("较上期：上期为 0 的 D 店、上期样本量不足的 E 店不下结论", by["D"]["verdict"] == "unknown" and by["E"]["verdict"] == "unknown", (by.get("D"), by.get("E")))
check("较上期：条件和窗口说明里写明本期 / 上期", "较上期变化" in d["condition"] and "上期" in (d["window"] or "") and "本期" in (d["window"] or ""), d)
s, r = call("POST", "/ledger/alerts/preview", A, body=rule(condition={"op": "lt", "threshold": -100, "basis": "change_abs"}, filters=[{"op": "in", "column": "店铺", "values": ["A", "B", "C"]}]))
by = {x["key"]: x for x in r["data"]["rows"]} if s == 200 else {}
check("较上期（绝对值）：A 少了 700，C 少了 700，B 没变", s == 200 and by["A"]["changeText"] == "-700.00" and by["C"]["changeText"] == "-700.00" and by["B"]["verdict"] == "clear", (s, r))

# ── 校验 ──
s, r = call("POST", "/ledger/alerts", A, body=rule(window={"dateColumn": None, "preset": "none"})); check("比较上期不设时间窗口被拒", s == 400 and "时间窗口" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/alerts", A, body=rule(window={"dateColumn": "日期", "preset": "this_month"})); check("比较上期选「本月」被拒（月初几天没法和整月比）", s == 400 and "昨天、近 7 天" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/alerts", A, body=rule(metric={"kind": "freshness", "dateColumn": "金额"}, window={"dateColumn": None, "preset": "none"}, guard=None, condition={"op": "gt", "threshold": 3})); check("数据停更选了非日期列被拒", s == 400 and "不是日期列" in msg((s, r)), (s, r))
FRESH = {"kind": "freshness", "dateColumn": "日期"}
s, r = call("POST", "/ledger/alerts", A, body=rule(metric=FRESH, condition={"op": "gt", "threshold": 3})); check("数据停更不能再设时间窗口", s == 400 and "时间窗口" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/alerts", A, body=rule(metric=FRESH, window={"dateColumn": None, "preset": "none"}, condition={"op": "gt", "threshold": 3})); check("数据停更不支持最小样本量", s == 400 and "样本量" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/alerts", A, body=rule(metric=FRESH, window={"dateColumn": None, "preset": "none"}, guard=None, condition={"op": "gt", "threshold": 3, "basis": "change_pct"})); check("数据停更不能比较上期", s == 400 and "上期" in msg((s, r)), (s, r))

# ── 创建：较上期，开启恢复通知 ──
s, r = call("POST", "/ledger/alerts", A, body=rule(notifyRecovery=True)); check("创建告警（较上期 + 恢复通知）", s == 200, (s, r))
AL = r["data"]
s, r = call("GET", "/ledger/alerts", A); item = next(x for x in r["data"]["items"] if x["id"] == AL)
check("列表里带出判断依据和恢复通知", item["condition"]["basis"] == "change_pct" and item["notifyRecovery"] is True, item)

s, r = call("POST", f"/ledger/alerts/{AL}/evaluate", A); run = r["data"] if s == 200 else r
check("第一次评估：命中 2 项、通知 2 项", s == 200 and run["status"] == "success" and run["hits"] == 2 and run["notified"] == 2, (s, r))
check("消息列出本期、上期、变化", len(HOOKS) == 1 and all(k in text_of(0) for k in ("本期", "上期", "变化", "-50.0%", "-100.0%", "较上期变化")), HOOKS)
check("消息里没有正常的 B 店", "B　" not in text_of(0) and "D　" not in text_of(0) and "E　" not in text_of(0), text_of(0))

s, r = call("POST", f"/ledger/alerts/{AL}/evaluate", A); run = r["data"]
check("第二次评估：仍然下滑，不重复通知", run["notified"] == 0 and len(HOOKS) == 1, (r, len(HOOKS)))

# ── 恢复通知：A 店本期补到和上期一样 ──
fix = [[f"A-now{i}", "A", day(1 + i), 200] for i in range(7)]
call("POST", f"/ledger/datasets/{DS}/writeback", A, body={"mode": "upsert", "columns": COLS, "rows": fix})
s, r = call("POST", f"/ledger/alerts/{AL}/evaluate", A); run = r["data"]
check("A 店恢复：发出「已恢复」通知，没有新的告警", run["status"] == "success" and run["notified"] == 0 and len(HOOKS) == 2, (r, len(HOOKS)))
check("恢复消息写明店铺和持续时间，不含仍在下滑的 C 店", "已恢复" in text_of(1) and "A" in text_of(1) and "共持续" in text_of(1) and "-100.0%" not in text_of(1), text_of(1) if len(HOOKS) > 1 else HOOKS)
check("运行记录说明恢复了几项", "1 项已恢复并通知" in (run["message"] or ""), run)
s, r = call("GET", "/ledger/alerts", A); item = next(x for x in r["data"]["items"] if x["id"] == AL)
check("列表：只剩 C 店触发中", item["firing"] == 1, item)
s, r = call("POST", f"/ledger/alerts/{AL}/evaluate", A); run = r["data"]
check("再评估一次：不会重复发恢复通知", len(HOOKS) == 2 and run["notified"] == 0, (r, len(HOOKS)))

# ── 不开恢复通知：恢复了也不发消息 ──
s, r = call("POST", "/ledger/alerts", A, body=rule(name="不通知恢复", filters=[{"op": "in", "column": "店铺", "values": ["C"]}])); QUIET = r["data"]
call("POST", f"/ledger/alerts/{QUIET}/evaluate", A)
n = len(HOOKS)
call("POST", f"/ledger/datasets/{DS}/writeback", A, body={"mode": "upsert", "columns": COLS, "rows": [[f"C-now{i}", "C", day(1 + i), 100] for i in range(7)]})
s, r = call("POST", f"/ledger/alerts/{QUIET}/evaluate", A); run = r["data"]
check("没开恢复通知：C 店恢复后不发消息，状态清掉", len(HOOKS) == n and run["hits"] == 0, (r, n, len(HOOKS)))
s, r = call("GET", "/ledger/alerts", A); q = next(x for x in r["data"]["items"] if x["id"] == QUIET)
check("没开恢复通知：触发状态照样清掉", q["firing"] == 0, q)

# ── 数据停更 ──
s, r = call("POST", "/ledger/datasets/input", A, body={"name": "订单流水", "columns": [
    {"title": "订单号", "kind": "text"}, {"title": "店铺", "kind": "text"}, {"title": "下单时间", "kind": "datetime"}], "primaryKey": ["订单号"]})
DS2 = r["data"]
ords = [[f"A{i}", "A", f"{day(1 + i)} 10:00:00"] for i in range(3)] + [[f"B{i}", "B", f"{day(5 + i)} 10:00:00"] for i in range(3)] + [["C0", "C", day(10)]]
call("POST", f"/ledger/datasets/{DS2}/writeback", A, body={"mode": "upsert", "columns": ["订单号", "店铺", "下单时间"], "rows": ords})
stale = lambda **kw: {
    "name": "店铺停更", "datasetId": DS2, "metric": {"kind": "freshness", "dateColumn": "下单时间"}, "groupBy": "店铺", "filters": [],
    "window": {"dateColumn": None, "preset": "none"}, "condition": {"op": "gt", "threshold": 3}, "guard": None,
    "schedule": {"kind": "manual"}, "channelIds": [CH], "repeat": {"kind": "until_recovered"}, "notifyRecovery": True, **kw}
s, r = call("POST", "/ledger/alerts/preview", A, body=stale())
d = r["data"] if s == 200 else r
by = {x["key"]: x for x in d["rows"]} if s == 200 else {}
check("数据停更：B 店 5 天、C 店 10 天没有新订单，A 店正常", s == 200 and d["hits"] == 2 and by["A"]["verdict"] == "clear" and by["B"]["verdict"] == "hit" and by["C"]["verdict"] == "hit", (s, r))
check("数据停更：显示天数和最新日期，最久的排最前", by["B"]["valueText"] == f"5 天（最新 {day(5)}）" and by["A"]["valueText"] == f"1 天（最新 {day(1)}）" and d["rows"][0]["key"] == "C", d["rows"])
check("数据停更：条件说明", "距今" in d["condition"] and "3 天" in d["condition"], d)
s, r = call("POST", "/ledger/alerts/preview", A, body=stale(groupBy=None, condition={"op": "gt", "threshold": 0}))
check("数据停更不分组：取整个数据集最新的一条（1 天前）", s == 200 and len(r["data"]["rows"]) == 1 and r["data"]["rows"][0]["valueText"].startswith("1 天"), (s, r))
s, r = call("POST", "/ledger/alerts/preview", A, body=stale(filters=[{"op": "in", "column": "店铺", "values": ["B"]}], groupBy=None))
check("数据停更可以先筛选再看（只看 B 店）", s == 200 and r["data"]["rows"][0]["valueText"].startswith("5 天"), (s, r))

s, r = call("POST", "/ledger/alerts", A, body=stale()); ST = r["data"]
s, r = call("POST", f"/ledger/alerts/{ST}/evaluate", A); run = r["data"] if s == 200 else r
check("停更告警：命中 2 个店铺并通知", s == 200 and run["hits"] == 2 and run["notified"] == 2, (s, r))
msg_idx = len(HOOKS) - 1
check("停更消息：列出店铺、天数、最新日期", all(k in text_of(msg_idx) for k in ("B", "C", "5 天", "10 天", f"最新 {day(10)}")), text_of(msg_idx))
# B 店有新订单了 → 恢复
call("POST", f"/ledger/datasets/{DS2}/writeback", A, body={"mode": "upsert", "columns": ["订单号", "店铺", "下单时间"], "rows": [["B-new", "B", f"{day(0)} 08:00:00"]]})
n = len(HOOKS)
s, r = call("POST", f"/ledger/alerts/{ST}/evaluate", A); run = r["data"]
check("B 店有新订单：发「已恢复」，C 店仍在停更", run["hits"] == 1 and len(HOOKS) == n + 1 and "已恢复" in text_of(n) and "B" in text_of(n) and "0 天" in text_of(n), (r, text_of(n) if len(HOOKS) > n else HOOKS))

# ── 修改时保留新字段 ──
s, r = call("GET", "/ledger/alerts", A); st_item = next(x for x in r["data"]["items"] if x["id"] == ST)
keys = ("name", "datasetId", "metric", "metricLabel", "groupBy", "filters", "window", "condition", "guard", "schedule", "channelIds", "repeat", "notifyRecovery", "enabled")
s, r = call("PUT", f"/ledger/alerts/{ST}", A, body={**{k: st_item[k] for k in keys}, "name": "店铺停更（改名）"})
check("改名不影响触发状态", s == 200, (s, r))
s, r = call("GET", "/ledger/alerts", A); st_item2 = next(x for x in r["data"]["items"] if x["id"] == ST)
check("改名后触发中的分组还在，恢复通知设置保留", st_item2["firing"] == 1 and st_item2["notifyRecovery"] is True, st_item2)
s, r = call("PUT", f"/ledger/alerts/{ST}", A, body={**{k: st_item[k] for k in keys}, "condition": {"op": "gt", "threshold": 7}})
s, r = call("GET", "/ledger/alerts", A); st_item3 = next(x for x in r["data"]["items"] if x["id"] == ST)
check("改阈值（判断条件变了），旧的触发状态作废", st_item3["firing"] == 0, st_item3)

for i in (AL, QUIET, ST):
    call("DELETE", f"/ledger/alerts/{i}", A)
finish()
