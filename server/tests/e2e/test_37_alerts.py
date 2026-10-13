# env: LEDGER_URL_IMPORT_ALLOW_LOCAL=1
"""阈值告警：试算、通知去重（新触发 / 持续 / 恢复后再触发）、发送失败重试、样本量保护、时间窗口、权限

场景：三家店铺的退款率。A 店 10%（超标）、B 店 2.5%（正常）、C 店 2 单退 1 单（50%，但样本量不足，不判断）。
本机起一个接收器当 Webhook（钉钉渠道），统计收到几条消息。
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

for t in (A, Bo, C, R): call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}

# ── 数据：订单（订单号、店铺、下单时间、成交额、退款额）──
today = datetime.date.today()
recent = (today - datetime.timedelta(days=2)).isoformat()
old = (today - datetime.timedelta(days=60)).isoformat()
s, r = call("POST", "/ledger/datasets/input", A, body={"name": "订单", "columns": [
    {"title": "订单号", "kind": "text"}, {"title": "店铺", "kind": "text"}, {"title": "下单时间", "kind": "datetime"},
    {"title": "成交额", "kind": "money"}, {"title": "退款额", "kind": "money"}], "primaryKey": ["订单号"]})
assert s == 200, (s, r)
DS = r["data"]
COLS = ["订单号", "店铺", "下单时间", "成交额", "退款额"]
rows = []
rows += [[f"A{i:03}", "A", recent, 100, 100 if i < 4 else 0] for i in range(40)]   # A：40 单，退 4 单 = 10%
rows += [[f"B{i:03}", "B", recent, 100, 100 if i < 1 else 0] for i in range(40)]   # B：40 单，退 1 单 = 2.5%
rows += [[f"C{i:03}", "C", recent, 100, 100 if i < 1 else 0] for i in range(2)]    # C：2 单，退 1 单 = 50%，样本太小
rows += [[f"Bold{i:02}", "B", old, 100, 100] for i in range(10)]                   # B 两个月前的 10 单全退：窗口之外
s, r = call("POST", f"/ledger/datasets/{DS}/writeback", A, body={"mode": "upsert", "columns": COLS, "rows": rows})
assert s == 200 and r["data"]["inserted"] == len(rows), (s, r)

# ── 渠道：只有管理员能建 ──
s, r = call("POST", "/ledger/report-channels", A, body={"name": "运营群", "kind": "dingtalk", "target": f"http://127.0.0.1:{HOOK_PORT}/hook"})
check("普通用户不能建推送渠道", s == 403, (s, r))
s, r = call("POST", "/ledger/report-channels", R, body={"name": "运营群", "kind": "dingtalk", "target": f"http://127.0.0.1:{HOOK_PORT}/hook"})
assert s == 200, (s, r)
CH = r["data"]
s, r = call("POST", "/ledger/report-channels", R, body={"name": "坏群", "kind": "dingtalk", "target": "http://127.0.0.1:1/hook"})
BAD_CH = r["data"]

RATE = {"kind": "ratio", "numerator": {"column": "退款额", "agg": "sum"}, "denominator": {"column": "成交额", "agg": "sum"}}
rule = lambda **kw: {
    "name": "店铺退款率偏高", "datasetId": DS, "metric": RATE, "metricLabel": "退款率", "groupBy": "店铺", "filters": [{"op": "in", "column": "店铺", "values": ["A", "B", "C"]}],
    "window": {"dateColumn": "下单时间", "preset": "last_7"}, "condition": {"op": "gt", "threshold": 0.05},
    "guard": {"agg": "count", "min": 30}, "schedule": {"kind": "manual"}, "channelIds": [CH], "repeat": {"kind": "until_recovered"}, **kw}

# ── 试算：不保存、不发送 ──
s, r = call("POST", "/ledger/alerts/preview", A, body=rule())
d = r["data"] if s == 200 else r
check("试算：只有 A 店超标", s == 200 and d["hits"] == 1 and d["rows"][0]["key"] == "A" and d["rows"][0]["valueText"] == "10.0%" and d["rows"][0]["verdict"] == "hit", (s, r))
check("试算：B 店正常（时间窗口排除了两个月前的退款），C 店样本不足不下结论", d["clear"] == 1 and d["unknown"] == 1 and {x["key"]: x["verdict"] for x in d["rows"]}["C"] == "unknown", d)
check("试算：说明条件与窗口", "退款率" in d["condition"] and "近 7 天" in (d["window"] or "") and d["guard"], d)
check("试算不发送", len(HOOKS) == 0, HOOKS)
s, r = call("POST", "/ledger/alerts/preview", A, body=rule(window={"dateColumn": None, "preset": "none"}))
check("不设时间窗口：B 店算进了旧退款，变成超标", s == 200 and {x["key"]: x["verdict"] for x in r["data"]["rows"]}["B"] == "hit", (s, r))
s, r = call("POST", "/ledger/alerts/preview", A, body=rule(guard=None))
check("不设最小样本量：2 单退 1 单的 C 店也会超标", s == 200 and {x["key"]: x["verdict"] for x in r["data"]["rows"]}["C"] == "hit", (s, r))
s, r = call("POST", "/ledger/alerts/preview", A, body=rule(groupBy=None, metric={"kind": "agg", "agg": "sum", "column": "退款额"}, condition={"op": "gt", "threshold": 100}, guard=None, window={"dateColumn": None, "preset": "none"}))
check("不分组：整体一个值", s == 200 and len(r["data"]["rows"]) == 1 and r["data"]["rows"][0]["key"] == "" and r["data"]["hits"] == 1, (s, r))

# ── 校验 ──
s, r = call("POST", "/ledger/alerts", A, body=rule(groupBy="不存在的列")); check("分组列不存在被拒", s == 400 and "没有" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/alerts", A, body=rule(datasetId="nope")); check("数据集不存在被拒", s == 400 and "不存在" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/alerts", A, body=rule(channelIds=["rc_nope"])); check("渠道不存在被拒", s == 400 and "渠道" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/alerts", A, body=rule(channelIds=[])); check("没有渠道被拒", s == 400, (s, r))
s, r = call("POST", "/ledger/alerts", A, body=rule(metric={"kind": "agg", "agg": "sum", "column": None})); check("求和不选列被拒", s == 400 and "需要选择一列" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/alerts", A, body=rule(window={"dateColumn": None, "preset": "last_7"})); check("设了时间窗口不选日期列被拒", s == 400 and "日期列" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/alerts", A, body=rule(name="  ")); check("名称为空被拒", s == 400, (s, r))

# ── 创建与通知 ──
s, r = call("POST", "/ledger/alerts", A, body=rule()); check("创建告警", s == 200, (s, r))
AL = r["data"]
s, r = call("GET", "/ledger/alerts", A); item = next(x for x in r["data"]["items"] if x["id"] == AL)
check("列表带创建人、数据集名，尚无触发", item["createdByName"] == "Alice" and item["datasetName"] == "订单" and item["firing"] == 0 and item["lastRun"] is None, item)

s, r = call("POST", f"/ledger/alerts/{AL}/evaluate", A); run = r["data"] if s == 200 else r
check("第一次评估：命中 1 项、通知 1 项", s == 200 and run["status"] == "success" and run["hits"] == 1 and run["notified"] == 1, (s, r))
check("接收器收到 1 条，内容含店铺、退款率、新触发", len(HOOKS) == 1 and "A" in text_of(0) and "10.0%" in text_of(0) and "新触发" in text_of(0) and "店铺退款率偏高" in text_of(0), HOOKS)
check("消息里没有正常的 B 店和样本不足的 C 店", "2.5%" not in text_of(0) and "50.0%" not in text_of(0), text_of(0))
check("消息写明统计范围（筛选）和触发条件、样本量要求", "店铺：A、B、C" in text_of(0) and "退款率 > 5.0%" in text_of(0) and "样本量" in text_of(0) and "近 7 天" in text_of(0), text_of(0))
s, r = call("GET", "/ledger/alerts", A); item = next(x for x in r["data"]["items"] if x["id"] == AL)
check("列表：1 项触发中", item["firing"] == 1 and item["lastRun"]["hits"] == 1, item)

s, r = call("POST", f"/ledger/alerts/{AL}/evaluate", A); run = r["data"]
check("第二次评估：仍超标，但不重复通知", run["hits"] == 1 and run["notified"] == 0 and len(HOOKS) == 1 and "都已通知过" in (run["message"] or ""), (r, len(HOOKS)))

# ── 恢复 → 再次超标重新通知 ──
fix = [[f"A{i:03}", "A", recent, 100, 0] for i in range(4)]
call("POST", f"/ledger/datasets/{DS}/writeback", A, body={"mode": "upsert", "columns": COLS, "rows": fix})
s, r = call("POST", f"/ledger/alerts/{AL}/evaluate", A); run = r["data"]
check("A 店恢复：没有命中，不发消息，触发状态清掉", run["hits"] == 0 and run["notified"] == 0 and len(HOOKS) == 1, (r, len(HOOKS)))
check("没有命中时的说明不能写成「都已通知过」", "都已通知过" not in (run["message"] or "") and "没有超出条件" in (run["message"] or ""), run)
s, r = call("GET", "/ledger/alerts", A); item = next(x for x in r["data"]["items"] if x["id"] == AL)
check("列表：触发中归零", item["firing"] == 0, item)
broken = [[f"A{i:03}", "A", recent, 100, 100] for i in range(4)]
call("POST", f"/ledger/datasets/{DS}/writeback", A, body={"mode": "upsert", "columns": COLS, "rows": broken})
s, r = call("POST", f"/ledger/alerts/{AL}/evaluate", A); run = r["data"]
check("恢复后再次超标：重新通知", run["notified"] == 1 and len(HOOKS) == 2 and "新触发" in text_of(1), (r, len(HOOKS)))

# ── 重复策略 ──
s, r = call("GET", "/ledger/alerts", A); item = next(x for x in r["data"]["items"] if x["id"] == AL)
upd = lambda **kw: call("PUT", f"/ledger/alerts/{AL}", A, body={**{k: item[k] for k in ("name", "datasetId", "metric", "metricLabel", "groupBy", "filters", "window", "condition", "guard", "schedule", "channelIds", "repeat", "enabled")}, **kw})
s, r = upd(repeat={"kind": "every_run"}); check("改成每次都通知", s == 200, (s, r))
s, r = call("POST", f"/ledger/alerts/{AL}/evaluate", A); run = r["data"]
check("每次都通知：持续超标也再发一条，标注持续时间", run["notified"] == 1 and len(HOOKS) == 3 and "持续" in text_of(2), (r, len(HOOKS)))
s, r = upd(repeat={"kind": "every_hours", "hours": 24}); check("改成每隔 24 小时", s == 200, (s, r))
s, r = call("POST", f"/ledger/alerts/{AL}/evaluate", A); run = r["data"]
check("每隔 24 小时：刚通知过，不重复", run["notified"] == 0 and len(HOOKS) == 3, (r, len(HOOKS)))
s, r = upd(repeat={"kind": "every_hours", "hours": 0}); check("间隔 0 小时被拒", s == 400, (s, r))
s, r = upd(condition={"op": "gt", "threshold": 0.5}); check("改阈值（定义变了）", s == 200, (s, r))
s, r = call("GET", "/ledger/alerts", A); item2 = next(x for x in r["data"]["items"] if x["id"] == AL)
check("改了判断条件，旧的触发状态作废", item2["firing"] == 0, item2)
s, r = call("POST", f"/ledger/alerts/{AL}/evaluate", A); check("阈值 50% 下没有命中（A 店 10%，C 店样本不足）", r["data"]["hits"] == 0 and len(HOOKS) == 3, (r, len(HOOKS)))

# ── 发送失败：不记状态，下次重试 ──
s, r = call("POST", "/ledger/alerts", A, body=rule(name="发往坏群", channelIds=[BAD_CH])); BAD_AL = r["data"]
s, r = call("POST", f"/ledger/alerts/{BAD_AL}/evaluate", A); run = r["data"]
check("所有渠道都发送失败：记失败，不算已通知", run["status"] == "failed" and run["notified"] == 0 and run["results"] and not run["results"][0]["ok"], (s, r))
s, r = call("GET", "/ledger/alerts", A); bad = next(x for x in r["data"]["items"] if x["id"] == BAD_AL)
check("没通知出去就不记触发状态（下次还会当作新触发重试）", bad["firing"] == 0 and bad["lastRun"]["status"] == "failed", bad)

# ── 记录 ──
s, r = call("GET", f"/ledger/alerts/{AL}/runs", A); runs = r["data"]
check("评估记录：新的在前，含命中数和通知数", s == 200 and len(runs) >= 7 and runs[0]["trigger"] == "manual" and all("hits" in x and "notified" in x for x in runs), (s, runs[:2]))

# ── 权限 ──
call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["A", "B"]})
s, r = call("GET", "/ledger/alerts", Bo); check("受限用户看不到告警配置", s == 403, (s, r))
s, r = call("POST", "/ledger/alerts", Bo, body=rule()); check("受限用户不能建告警（否则能绕过自己的数据范围）", s == 403, (s, r))
s, r = call("POST", "/ledger/alerts/preview", Bo, body=rule()); check("受限用户不能试算", s == 403, (s, r))
s, r = call("GET", "/ledger/alerts", C); check("不受限的普通用户可以看到列表", s == 200 and any(x["id"] == AL for x in r["data"]["items"]), (s, r))
s, r = call("PUT", f"/ledger/alerts/{AL}", C, body={k: item[k] for k in ("name", "datasetId", "metric", "groupBy", "filters", "window", "condition", "guard", "schedule", "channelIds", "repeat", "enabled")}); check("别人建的告警，Carol 不能改", s == 403, (s, r))
s, r = call("DELETE", f"/ledger/alerts/{AL}", C); check("也不能删", s == 403, (s, r))
s, r = call("DELETE", f"/ledger/alerts/{BAD_AL}", R); check("管理员可以删别人的告警", s == 200, (s, r))
s, r = call("DELETE", f"/ledger/alerts/{AL}", A); check("创建人可以删自己的告警", s == 200, (s, r))
s, r = call("GET", "/ledger/alerts", A); check("删除后列表为空", s == 200 and r["data"]["items"] == [], (s, r))
s, r = call("GET", f"/ledger/alerts/{AL}/runs", A); check("评估记录一并删除", s == 200 and r["data"] == [], (s, r))
s, r = call("POST", f"/ledger/alerts/{AL}/evaluate", A); check("删除后不能再评估", s == 400 and "不存在" in msg((s, r)), (s, r))

# ── 审计：写操作都记 ──
time.sleep(3)  # 审计按 2 秒一批异步写入
s, r = call("GET", "/ledger/audit?size=200", R)
actions = " ".join(x.get("action", "") for x in r["data"]["items"]) if s == 200 else str(r)
check("审计里有告警的新建、修改、删除", all(k in actions for k in ("新建告警", "修改告警", "删除告警")), actions[:300])

finish()
