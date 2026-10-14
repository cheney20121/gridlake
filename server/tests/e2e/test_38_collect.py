# env: LEDGER_URL_IMPORT_ALLOW_LOCAL=1
"""多人填报：收集任务（任务与期次、负责人的数据范围、提交校验与状态看板、我的填报、权限）

场景：四家店铺每月报经营数据。店铺 A、D 归 Bob，店铺 B 归 Carol，店铺 C 没有指派（管理员代填）。
所有人往同一个数据集里填，各填各的行；看板按单位 × 期次显示谁填了。
"""
import datetime
import http.server
import threading

from lib import *  # noqa: F403

# 本机起一个接收器当群机器人 Webhook，统计催办推送了几条
HOOK_PORT = int(os.environ.get("LEDGER_E2E_HOOK_PORT", "19004"))
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


threading.Thread(target=http.server.ThreadingHTTPServer(("127.0.0.1", HOOK_PORT), Hook).serve_forever, daemon=True).start()

for t in (A, Bo, C, R): call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}

COLS = ["填报单位", "期次", "营收"]
wb = lambda tok, ds, rows, cols=COLS, mode="upsert": call("POST", f"/ledger/datasets/{ds}/writeback", tok, body={"mode": mode, "columns": cols, "rows": rows})
row = lambda unit, period, v: [unit, period, v]
cursor_total = lambda tok, ds: call("POST", f"/ledger/datasets/{ds}/cursor", tok, body={"filters": [], "keyword": None, "sort": [], "rule": None})[1]["data"]["total"]

# ── 创建 ──
body = {
    "name": "月度经营", "newTable": {"name": "月度经营表", "columns": [{"title": "营收", "kind": "money"}, {"title": "备注", "kind": "text"}], "keyColumns": []},
    "units": [{"unit": "店铺A", "userId": ids["Bob"]}, {"unit": "店铺B", "userId": ids["Carol"]}, {"unit": "店铺C"}, {"unit": "店铺D", "userId": ids["Bob"]}],
}
s, r = call("POST", "/ledger/collects", A, body=body); check("普通用户不能建收集任务（会改别人的数据权限）", s == 403, (s, r))
s, r = call("GET", "/ledger/collects", A); check("普通用户看不到任务管理", s == 403, (s, r))
for label, bad, word in [
    ("单位重复", {**body, "units": [{"unit": "X"}, {"unit": " X "}]}, "重复"),
    ("没有单位", {**body, "units": []}, "至少添加一个"),
    ("负责人没登录过", {**body, "units": [{"unit": "X", "userId": "nobody"}]}, "还没有登录过"),
    ("没写任务名", {**body, "name": " "}, "任务名称"),
    ("数据集和新建填报表二选一", {**body, "datasetId": "ds_x"}, "二选一"),
    ("没有要填写的列", {**body, "newTable": {"name": "空表", "columns": [], "keyColumns": []}}, "至少添加一个要填写的列"),
]:
    s, r = call("POST", "/ledger/collects", R, body=bad); check(f"创建被拒：{label}", s == 400 and word in msg((s, r)), (s, r))
s, r = call("GET", "/ledger/collects", R); check("被拒的创建不留下任务", s == 200 and r["data"] == [], (s, r))

s, r = call("POST", "/ledger/collects", R, body=body); check("管理员新建任务（同时建填报表）", s == 200, (s, r))
CO = r["data"]
s, r = call("GET", "/ledger/collects", R); item = r["data"][0]
DS = item["datasetId"]
check("列表：4 个单位、3 个已指派、还没有期次", item["unitCount"] == 4 and item["assignedCount"] == 3 and item["periodCount"] == 0 and item["latestPeriod"] is None and item["datasetName"] == "月度经营表", item)
check("单位列、期次列、权限维度用默认值", (item["unitColumn"], item["periodColumn"], item["dimension"]) == ("填报单位", "期次", "填报单位"), item)

# ── 还没开启期次 ──
s, r = wb(Bo, DS, [row("店铺A", "2026-10", 100)]); check("没开启任何一期：不能提交", s == 400 and "还没有开启任何一期" in msg((s, r)), (s, r))
s, r = call("POST", f"/ledger/collects/{CO}/periods", A, body={"period": "2026-10"}); check("普通用户不能开启期次", s == 403, (s, r))
s, r = call("POST", f"/ledger/collects/{CO}/periods", R, body={"period": "2026/10"}); check("期次名不能含斜杠", s == 400, (s, r))
s, r = call("POST", f"/ledger/collects/{CO}/periods", R, body={"period": "2026-10"}); check("开启 2026-10", s == 200, (s, r))
s, r = call("POST", f"/ledger/collects/{CO}/periods", R, body={"period": "2026-10"}); check("重复开启被拒", s == 400 and "已经开启" in msg((s, r)), (s, r))

# ── 提交校验 ──
s, r = wb(Bo, DS, [row("店铺A", "2026-11", 1)]); check("期次没开启被拒，并说明现在可以填哪些", s == 400 and "2026-11" in msg((s, r)) and "现在可以填：2026-10" in msg((s, r)), (s, r))
s, r = wb(Bo, DS, [row("店铺Z", "2026-10", 1)]); check("单位不在任务里被拒", s == 400 and "店铺Z" in msg((s, r)) and "不在收集任务" in msg((s, r)), (s, r))
s, r = wb(Bo, DS, [["店铺A", 1]], cols=["填报单位", "营收"]); check("填报区缺期次列被拒", s == 400 and "必须包含" in msg((s, r)), (s, r))
s, r = wb(Bo, DS, [[None, "2026-10", 1]]); check("单位为空被拒", s == 400 and "（空）" in msg((s, r)), (s, r))
s, r = wb(Bo, DS, [row("店铺A", "2026-10", 100)]); check("Bob 提交自己负责的店铺 A", s == 200 and r["data"]["inserted"] == 1 and r["data"]["batchId"], (s, r))
s, r = wb(Bo, DS, [row("店铺B", "2026-10", 5)]); d = r["data"] if s == 200 else r
check("Bob 提交 Carol 的店铺 B：范围外，整批不写入（不记为已提交）", s == 200 and d["batchId"] is None and "不在你的数据范围内" in d["errors"][0]["message"], (s, r))

# ── 看板 ──
s, r = call("GET", f"/ledger/collects/{CO}", R); d = r["data"]
by = {u["unit"]: u for u in d["units"]}
check("看板默认最近一期", s == 200 and d["period"] == "2026-10" and [p["period"] for p in d["periods"]] == ["2026-10"], d)
check("A 已提交（Bob，1 行），B、C、D 未提交", (by["店铺A"]["status"], by["店铺A"]["submittedByName"], by["店铺A"]["rows"]) == ("submitted", "Bob", 1) and {by[u]["status"] for u in ("店铺B", "店铺C", "店铺D")} == {"pending"}, d["units"])
check("看板带负责人，未指派的为空", by["店铺A"]["userName"] == "Bob" and by["店铺C"]["userName"] is None, d["units"])
check("期次进度 1/4", d["periods"][0]["submitted"] == 1 and d["periods"][0]["total"] == 4 and d["collect"]["latestSubmitted"] == 1, d["periods"])
s, r = call("GET", f"/ledger/collects/{CO}?period=2099-01", R); check("看不存在的期次被拒", s == 400 and "没有开启" in msg((s, r)), (s, r))
s, r = call("GET", f"/ledger/collects/{CO}", Bo); check("受限用户看不到看板", s == 403, (s, r))

# ── 我的填报 ──
s, r = call("GET", "/ledger/collects/mine", Bo); mine = r["data"]
check("受限的负责人也能看「我的填报」：Bob 有 A、D 两个单位，D 待填", s == 200 and len(mine) == 1 and [u["unit"] for u in mine[0]["units"]] == ["店铺A", "店铺D"] and mine[0]["pending"] == 1, (s, r))
check("我的填报带数据集名和列名，方便说明怎么填", mine[0]["datasetName"] == "月度经营表" and mine[0]["unitColumn"] == "填报单位", mine[0])
s, r = call("GET", "/ledger/collects/mine", C); check("Carol 只看到自己的店铺 B", len(r["data"][0]["units"]) == 1 and r["data"][0]["units"][0]["unit"] == "店铺B", r)
s, r = call("GET", "/ledger/collects/mine", A); check("没被指派的人什么都没有", s == 200 and r["data"] == [], (s, r))
s, r = call("GET", "/ledger/collects", Bo); check("负责人是受限用户：不能管理任务", s == 403, (s, r))
s, r = call("POST", "/ledger/alerts", Bo, body={}); check("负责人是受限用户：告警等绕过范围的功能也关闭", s == 403, (s, r))

# ── 数据范围：各填各的、各看各的 ──
s, r = wb(C, DS, [row("店铺B", "2026-10", 50)]); check("Carol 提交自己的店铺 B", s == 200 and r["data"]["inserted"] == 1, (s, r))
s, r = wb(Bo, DS, [row("店铺D", "2026-10", 40)]); check("Bob 提交店铺 D", s == 200, (s, r))
s, r = wb(R, DS, [row("店铺C", "2026-10", 30)]); check("管理员代填没有指派负责人的店铺 C", s == 200 and r["data"]["inserted"] == 1, (s, r))
s, r = call("GET", f"/ledger/collects/{CO}", R); d = r["data"]; by = {u["unit"]: u for u in d["units"]}
check("全部提交：4/4，C 的提交人是管理员", d["periods"][0]["submitted"] == 4 and by["店铺C"]["submittedByName"] == "Root" and by["店铺B"]["submittedByName"] == "Carol", d)
check("Bob 只看得到自己的两个单位（2 行），Carol 只看到 1 行，管理员看到全部 4 行", (cursor_total(Bo, DS), cursor_total(C, DS), cursor_total(R, DS)) == (2, 1, 4), (cursor_total(Bo, DS), cursor_total(C, DS), cursor_total(R, DS)))
s, r = call("GET", "/ledger/collects/mine", Bo); check("Bob 的待填清零", r["data"][0]["pending"] == 0, r)
s, r = wb(Bo, DS, [row("店铺A", "2026-10", 200)]); check("重新提交：按主键更新同一行", s == 200 and r["data"]["updated"] == 1 and r["data"]["inserted"] == 0, (s, r))
s, r = call("POST", f"/ledger/datasets/{DS}/aggregate", R, body={"calls": [{"func": "sum", "column": "营收", "criteria": [{"column": "填报单位", "value": "店铺A"}]}, {"func": "count", "criteria": []}]})
check("A 的营收更新为 200，数据集仍是 4 行（每个单位每期一行）", r["data"]["values"] == [200, 4], r)

# ── 一键生成填报工作簿 ──
def snapshot(tok, workbook_id):
    st, raw = call("GET", f"/ledger/workbooks/{workbook_id}/content", tok, binary=True)
    assert st == 200, (st, raw)
    return json.loads(gzip.decompress(raw))
gen = lambda tok, unit, period="2026-10", co=None: call("POST", f"/ledger/collects/{co or CO}/workbook", tok, body={"unit": unit, "period": period})
s, r = gen(Bo, "店铺A"); check("Bob 为自己负责的店铺 A 生成填报工作簿", s == 200 and r["data"]["created"] and r["data"]["workbookId"], (s, r))
FW = r["data"]["workbookId"]
s, r = gen(Bo, "店铺A"); check("再点一次：打开已有的，不重复生成", s == 200 and not r["data"]["created"] and r["data"]["workbookId"] == FW, (s, r))
s, r = gen(Bo, "店铺B"); check("不能为别人负责的单位生成", s == 403 and "不是你负责" in msg((s, r)), (s, r))
s, r = gen(Bo, "店铺Z"); check("单位不在任务里被拒", s == 400 and "不在收集任务" in msg((s, r)), (s, r))
s, r = gen(Bo, "店铺A", "2026-12"); check("期次没开启被拒", s == 400 and "没有开启" in msg((s, r)), (s, r))
s, r = gen(A, "店铺A"); check("没被指派的普通用户不能生成", s == 403, (s, r))
s, r = gen(R, "店铺A"); check("管理员可以为任何单位生成，各有各的一份", s == 200 and r["data"]["created"] and r["data"]["workbookId"] != FW, (s, r))
snap = snapshot(Bo, FW); sheet = snap["sheets"]["sheet-1"]; inp = sheet["custom"]["inputs"][0]
check("填报区只有业务列，单位和期次是固定列", inp["fields"] == ["营收", "备注"] and inp["fixed"] == {"填报单位": "店铺A", "期次": "2026-10"} and inp["datasetId"] == DS and inp["mode"] == "upsert", inp)
check("填报区定义的位置：第 4 行是表头", (inp["row"], inp["col"]) == (3, 0) and sheet["cellData"]["3"]["0"]["v"] == "营收" and sheet["cellData"]["3"]["1"]["v"] == "备注", (inp, sheet["cellData"].get("3")))
check("店铺 A 已经提交过的数据预填进去（营收 200）", sheet["cellData"]["4"]["0"]["v"] == 200, sheet["cellData"].get("4"))
check("工作簿名称带任务、单位、期次", snap["name"].startswith("月度经营 · 店铺A · 2026-10"), snap["name"])
s, r = call("GET", f"/ledger/workbooks/{FW}/content", C, binary=True); check("生成的工作簿默认仅自己可见（Carol 打不开）", s in (403, 404), s)
# 前端提交时：固定列 + 业务列，服务端按单位、期次记录
s, r = wb(Bo, DS, [["店铺A", "2026-10", 300, "改了"]], cols=["填报单位", "期次", "营收", "备注"]); check("按生成的填报区提交（固定列 + 业务列）", s == 200 and r["data"]["updated"] == 1, (s, r))
s, r = call("POST", f"/ledger/datasets/{DS}/aggregate", R, body={"calls": [{"func": "sum", "column": "营收", "criteria": [{"column": "填报单位", "value": "店铺A"}]}]})
check("店铺 A 的营收更新为 300", r["data"]["values"][0] == 300, r)
s, r = gen(C, "店铺B"); check("Carol 为店铺 B 生成，预填她已提交的 50", s == 200 and snapshot(C, r["data"]["workbookId"])["sheets"]["sheet-1"]["cellData"]["4"]["0"]["v"] == 50, (s, r))

# ── 期次管理 ──
s, r = call("DELETE", f"/ledger/collects/{CO}/periods/2026-10", R); check("有人提交过的期次不能删", s == 400 and "不能删除" in msg((s, r)), (s, r))
call("POST", f"/ledger/collects/{CO}/periods", R, body={"period": "2026-11"})
s, r = call("GET", f"/ledger/collects/{CO}", R); d = r["data"]
check("新开的一期成为最近一期，全部待填", d["period"] == "2026-11" and d["periods"][0]["submitted"] == 0 and {u["status"] for u in d["units"]} == {"pending"}, d)
s, r = call("GET", "/ledger/collects/mine", Bo); check("Bob 又有 2 项待填（A、D 的 2026-11）", r["data"][0]["pending"] == 2, r)
s, r = call("DELETE", f"/ledger/collects/{CO}/periods/2026-11", R); check("没人提交的期次可以删", s == 200, (s, r))
s, r = call("DELETE", f"/ledger/collects/{CO}/periods/2026-11", R); check("删了再删：没有这一期", s == 400, (s, r))

# ── 修改单位与负责人：数据范围跟着变 ──
upd = {"name": "月度经营数据", "units": [{"unit": "店铺A", "userId": ids["Alice"]}, {"unit": "店铺B", "userId": ids["Carol"]}, {"unit": "店铺C"}]}
s, r = call("PUT", f"/ledger/collects/{CO}", A, body=upd); check("普通用户不能改任务", s == 403, (s, r))
s, r = call("PUT", f"/ledger/collects/{CO}", R, body=upd); check("店铺 A 换给 Alice，店铺 D 去掉", s == 200, (s, r))
s, r = call("GET", f"/ledger/collects/{CO}", R); d = r["data"]; by = {u["unit"]: u for u in d["units"]}
check("看板更新：3 个单位，A 的负责人是 Alice，提交记录保留", d["collect"]["name"] == "月度经营数据" and set(by) == {"店铺A", "店铺B", "店铺C"} and by["店铺A"]["userName"] == "Alice" and by["店铺A"]["status"] == "submitted", d)
check("Bob 名下没有单位了，回到不受限（能看到整个数据集）；Alice 成为受限用户只看到 A", (cursor_total(Bo, DS), cursor_total(A, DS)) == (4, 1), (cursor_total(Bo, DS), cursor_total(A, DS)))
s, r = call("GET", "/ledger/collects/mine", Bo); check("Bob 的我的填报里没有任务了", r["data"] == [], r)
s, r = call("POST", "/ledger/alerts", A, body={}); check("被指派后 Alice 也成了受限用户，告警等功能关闭", s == 403, (s, r))
s, r = call("PUT", f"/ledger/collects/{CO}", R, body={"name": "x", "units": [{"unit": "A", "userId": "nobody"}]}); check("改单位时负责人也要登录过", s == 400, (s, r))

# ── 审核：确认人、确认锁定、退回重填 ──
units_now = [{"unit": "店铺A", "userId": ids["Alice"]}, {"unit": "店铺B", "userId": ids["Carol"]}, {"unit": "店铺C"}]
put = lambda reviewer, units=None: call("PUT", f"/ledger/collects/{CO}", R, body={"name": "月度经营数据", "reviewerId": reviewer, "units": units or units_now})
review = lambda tok, action, units, reason=None, period="2026-10": call("POST", f"/ledger/collects/{CO}/review", tok, body={"period": period, "units": units, "action": action, **({"reason": reason} if reason else {})})
status = lambda: {u["unit"]: u["status"] for u in call("GET", f"/ledger/collects/{CO}", R)[1]["data"]["units"]}
inbox = lambda tok: call("GET", "/ledger/notifications", tok)[1]["data"]["items"]

s, r = put(ids["Alice"]); check("确认人不能同时是某个单位的负责人（看不到全部数据没法确认）", s == 400 and "同时是填报单位「店铺A」的负责人" in msg((s, r)), (s, r))
s, r = put("nobody"); check("确认人要登录过系统", s == 400 and "确认人还没有登录过" in msg((s, r)), (s, r))
s, r = put(ids["Bob"]); check("指定 Bob 为确认人", s == 200, (s, r))
s, r = call("GET", "/ledger/collects", R); check("任务列表带确认人", r["data"][0]["reviewerName"] == "Bob" and r["data"][0]["latestConfirmed"] == 0, r)
s, r = put(ids["Alice"], [{"unit": "店铺A", "userId": ids["Alice"]}]); check("改任务：确认人冲突被拒，原来的确认人不变", s == 400, (s, r))
check("…而且单位没有被改掉", set(status()) == {"店铺A", "店铺B", "店铺C"}, status())

# 权限：看板和审核只有管理员和确认人
s, r = call("GET", f"/ledger/collects/{CO}", A); check("受限的负责人看不到看板", s == 403 and "确认人" in msg((s, r)), (s, r))
s, r = review(A, "confirm", ["店铺A"]); check("负责人不能给自己确认", s == 403, (s, r))
s, r = review(C, "confirm", ["店铺B"]); check("Carol 也不能", s == 403, (s, r))
s, r = call("GET", f"/ledger/collects/{CO}", Bo); check("确认人 Bob 能看板，但不能管理期次", s == 200 and r["data"]["canManage"] is False and r["data"]["period"] == "2026-10", (s, r))
s, r = call("GET", f"/ledger/collects/{CO}", R); check("管理员能管理", r["data"]["canManage"] is True, r)
s, r = call("POST", f"/ledger/collects/{CO}/periods", Bo, body={"period": "2026-12"}); check("确认人不能开启期次", s == 403, (s, r))
s, r = call("PUT", f"/ledger/collects/{CO}", Bo, body={"name": "x", "units": units_now}); check("确认人不能改任务", s == 403, (s, r))
s, r = call("GET", "/ledger/collects/mine", Bo); m = r["data"][0]
check("Bob 的我的填报：他是确认人，3 项等他确认，自己没有要填的单位", s == 200 and m["reviewer"] and m["toReview"] == 3 and m["units"] == [] and m["pending"] == 0, (s, r))
s, r = call("GET", "/ledger/collects/mine", C); check("Carol 看得到确认人是谁", r["data"][0]["reviewerName"] == "Bob" and not r["data"][0]["reviewer"], r)

# 确认并锁定
s, r = review(Bo, "confirm", ["店铺C", "店铺Z"]); check("单位不在任务里被拒", s == 400 and "不在收集任务" in msg((s, r)), (s, r))
s, r = review(Bo, "confirm", ["店铺A"], period="2099-01"); check("期次没开启被拒", s == 400 and "没有开启" in msg((s, r)), (s, r))
s, r = review(Bo, "lock", ["店铺A"]); check("操作不对被拒", s == 400, (s, r))
s, r = review(Bo, "confirm", []); check("没选单位被拒", s == 400, (s, r))
s, r = review(Bo, "confirm", ["店铺A", "店铺B"]); check("Bob 确认店铺 A、B", s == 200 and r["data"]["changed"] == 2, (s, r))
check("A、B 已确认，C 还在等确认", status() == {"店铺A": "confirmed", "店铺B": "confirmed", "店铺C": "submitted"}, status())
s, r = call("GET", f"/ledger/collects/{CO}", R); d = r["data"]
check("期次进度：已提交 3，已确认 2；确认人和时间记下来", d["periods"][0]["submitted"] == 3 and d["periods"][0]["confirmed"] == 2 and d["collect"]["latestConfirmed"] == 2 and next(u for u in d["units"] if u["unit"] == "店铺A")["reviewedByName"] == "Bob", d)
s, r = review(Bo, "confirm", ["店铺A"]); check("已经确认的不能重复确认", s == 400 and "已经确认锁定" in msg((s, r)), (s, r))
s, r = review(Bo, "confirm", ["店铺C", "店铺A"]); check("批量里有一个不行：整批不变（C 仍是待确认）", s == 400 and status()["店铺C"] == "submitted", (s, r))
check("负责人收到确认通知", any(n["kind"] == "collect_confirmed" and "店铺B" in n["text"] and n["fromName"] == "Bob" for n in inbox(C)), inbox(C))

s, r = wb(A, DS, [row("店铺A", "2026-10", 999)]); check("锁定后负责人不能再提交", s == 400 and "店铺A · 2026-10" in msg((s, r)) and "已经确认锁定" in msg((s, r)), (s, r))
s, r = wb(R, DS, [row("店铺A", "2026-10", 999)]); check("管理员也一样（要改得先退回 / 撤销确认）", s == 400 and "已经确认锁定" in msg((s, r)), (s, r))
s, r = wb(A, DS, [["店铺A", "2026-10"]], cols=["填报单位", "期次"], mode="delete"); check("锁定后也不能按主键删除", s == 400 and "已经确认锁定" in msg((s, r)), (s, r))
s, r = call("POST", f"/ledger/datasets/{DS}/aggregate", R, body={"calls": [{"func": "sum", "column": "营收", "criteria": [{"column": "填报单位", "value": "店铺A"}]}]})
check("锁定的数据没有变（店铺 A 仍是 300）", r["data"]["values"][0] == 300, r)
s, r = wb(R, DS, [row("店铺C", "2026-10", 35)]); check("别的单位不受影响：管理员改店铺 C", s == 200 and r["data"]["updated"] == 1, (s, r))

# 退回重填
s, r = review(Bo, "return", ["店铺B"]); check("退回要写原因", s == 400 and "原因" in msg((s, r)), (s, r))
s, r = review(Bo, "return", ["店铺B"], "错" * 201); check("退回原因不能太长", s == 400, (s, r))
s, r = review(Bo, "return", ["店铺B"], "营收口径不对，按含税填"); check("Bob 把已确认的店铺 B 退回", s == 200, (s, r))
check("B 已退回，不算已提交", status()["店铺B"] == "returned" and call("GET", f"/ledger/collects/{CO}", R)[1]["data"]["periods"][0]["submitted"] == 2, status())
s, r = call("GET", f"/ledger/collects/{CO}", R); b = next(u for u in r["data"]["units"] if u["unit"] == "店铺B")
check("看板带退回原因和确认人", b["returnReason"] == "营收口径不对，按含税填" and b["reviewedByName"] == "Bob", b)
s, r = review(Bo, "return", ["店铺B"], "再来一次"); check("已退回的不能再退回", s == 400 and "已经退回" in msg((s, r)), (s, r))
s, r = review(Bo, "confirm", ["店铺B"]); check("退回后没重新提交前不能确认", s == 400 and "已经退回" in msg((s, r)), (s, r))
s, r = call("GET", "/ledger/collects/mine", C); t = r["data"][0]; p = t["units"][0]["periods"][0]
check("Carol 的我的填报：B 已退回并带原因，算 1 项待填", (p["status"], p["returnReason"], t["pending"]) == ("returned", "营收口径不对，按含税填", 1), t)
check("Carol 收到退回通知（带原因）", any(n["kind"] == "collect_returned" and "按含税填" in n["text"] and "店铺B" in n["text"] for n in inbox(C)), inbox(C))
s, r = wb(C, DS, [row("店铺B", "2026-10", 55)]); check("退回后 Carol 可以改了重新提交", s == 200 and r["data"]["updated"] == 1, (s, r))
check("重新提交后回到待确认，退回记录清除", status()["店铺B"] == "submitted", status())
s, r = call("GET", "/ledger/collects/mine", C); check("Carol 的待填清零", r["data"][0]["pending"] == 0 and r["data"][0]["units"][0]["periods"][0]["returnReason"] is None, r)
s, r = review(Bo, "confirm", ["店铺B"]); check("确认人再次确认 B", s == 200 and status()["店铺B"] == "confirmed", (s, r))

# 撤销确认
s, r = review(Bo, "reopen", ["店铺C"]); check("没确认的不能撤销", s == 400 and "正在等确认" in msg((s, r)), (s, r))
s, r = review(R, "confirm", ["店铺C"]); check("管理员也能确认（C 没有负责人）", s == 200, (s, r))
s, r = review(R, "reopen", ["店铺A"]); check("撤销店铺 A 的确认", s == 200 and status()["店铺A"] == "submitted", (s, r))
s, r = wb(A, DS, [row("店铺A", "2026-10", 310)]); check("撤销后负责人又能改了", s == 200 and r["data"]["updated"] == 1, (s, r))
s, r = call("GET", "/ledger/collects/mine", Bo); check("确认人待确认：只剩 A", r["data"][0]["toReview"] == 1, r)

# ── 截止日期与催办 ──
day = lambda n: (datetime.date.today() + datetime.timedelta(days=n)).isoformat()
s, r = call("POST", "/ledger/report-channels", R, body={"name": "填报群", "kind": "dingtalk", "target": f"http://127.0.0.1:{HOOK_PORT}/hook"}); CH = r["data"]
remind = lambda tok, period="2026-12", units=None: call("POST", f"/ledger/collects/{CO}/remind", tok, body={"period": period, **({"units": units} if units else {})})
s, r = call("POST", f"/ledger/collects/{CO}/periods", R, body={"period": "2026-12", "deadline": "2026/12/31"}); check("截止日期格式不对被拒（期次也没有开启）", s == 400 and "年-月-日" in msg((s, r)), (s, r))
s, r = call("POST", f"/ledger/collects/{CO}/periods", A, body={"period": "2026-12"}); check("普通用户不能开启期次", s == 403, (s, r))
s, r = call("POST", f"/ledger/collects/{CO}/periods", R, body={"period": "2026-12", "deadline": day(-1)}); check("开启 2026-12，截止日期是昨天（已经逾期）", s == 200, (s, r))
s, r = call("GET", f"/ledger/collects/{CO}", R); d = r["data"]; p12 = next(p for p in d["periods"] if p["period"] == "2026-12")
check("看板：这一期的截止日期和逾期单位数（3 个都没交）", p12["deadline"] == day(-1) and p12["overdue"] == 3 and d["period"] == "2026-12" and all(u["overdue"] for u in d["units"]), d)
check("任务列表带最近一期的截止日期和逾期数", call("GET", "/ledger/collects", R)[1]["data"][0]["latestDeadline"] == day(-1) and call("GET", "/ledger/collects", R)[1]["data"][0]["latestOverdue"] == 3, call("GET", "/ledger/collects", R)[1]["data"])
s, r = call("PUT", f"/ledger/collects/{CO}/periods/2026-12", A, body={"deadline": day(3)}); check("普通用户不能改截止日期", s == 403, (s, r))
s, r = call("PUT", f"/ledger/collects/{CO}/periods/2026-12", Bo, body={"deadline": day(3)}); check("确认人也不能改（只有管理员管理期次）", s == 403, (s, r))
s, r = call("PUT", f"/ledger/collects/{CO}/periods/2026-12", R, body={"deadline": "明天"}); check("改截止日期：格式不对被拒", s == 400 and "年-月-日" in msg((s, r)), (s, r))
s, r = call("PUT", f"/ledger/collects/{CO}/periods/2099-01", R, body={"deadline": day(3)}); check("没有这一期", s == 400 and "没有这一期" in msg((s, r)), (s, r))
s, r = call("PUT", f"/ledger/collects/{CO}/periods/2026-12", R, body={"deadline": day(3)}); check("延期到 3 天后", s == 200, (s, r))
d = call("GET", f"/ledger/collects/{CO}", R)[1]["data"]; p12 = next(p for p in d["periods"] if p["period"] == "2026-12")
check("延期后不再逾期", p12["deadline"] == day(3) and p12["overdue"] == 0 and not any(u["overdue"] for u in d["units"]), p12)
s, r = call("PUT", f"/ledger/collects/{CO}/periods/2026-12", R, body={"deadline": None}); check("取消截止日期", s == 200 and next(p for p in call("GET", f"/ledger/collects/{CO}", R)[1]["data"]["periods"] if p["period"] == "2026-12")["deadline"] is None, (s, r))
call("PUT", f"/ledger/collects/{CO}/periods/2026-12", R, body={"deadline": day(-1)})
s, r = call("GET", "/ledger/collects/mine", A); t = r["data"][0]; p = next(p for u in t["units"] for p in u["periods"] if p["period"] == "2026-12")
check("Alice 的我的填报：这一期逾期，带截止日期", p["overdue"] and p["deadline"] == day(-1) and t["overdue"] == 1, t)

# 催办
s, r = call("PUT", f"/ledger/collects/{CO}", R, body={"name": "月度经营数据", "reviewerId": ids["Bob"], "remindChannels": ["nope"], "units": units_now}); check("催办渠道必须存在", s == 400 and "推送渠道不存在" in msg((s, r)), (s, r))
s, r = call("PUT", f"/ledger/collects/{CO}", R, body={"name": "月度经营数据", "reviewerId": ids["Bob"], "remindChannels": [CH], "units": units_now}); check("任务配置催办推送到填报群", s == 200, (s, r))
check("任务列表带催办渠道", call("GET", "/ledger/collects", R)[1]["data"][0]["remindChannels"] == [CH], call("GET", "/ledger/collects", R)[1]["data"])
s, r = remind(A); check("负责人不能催办", s == 403, (s, r))
s, r = remind(C); check("别的负责人也不能", s == 403, (s, r))
s, r = remind(Bo, "2099-01"); check("催办：期次没开启被拒", s == 400 and "没有开启" in msg((s, r)), (s, r))
s, r = remind(Bo, "2026-12", ["店铺Z"]); check("催办：单位不在任务里被拒", s == 400 and "不在收集任务" in msg((s, r)), (s, r))
s, r = remind(Bo, "2026-10"); check("这一期都提交了：没有要催的", s == 400 and "都已提交" in msg((s, r)), (s, r))
HOOKS.clear()
s, r = remind(Bo); o = r["data"] if s == 200 else r
check("确认人催办：通知 A、B 两位负责人，C 没有负责人，名单推送到群", s == 200 and (o["notified"], o["skipped"], o["unassigned"]) == (2, 0, 1) and len(o["channels"]) == 1 and o["channels"][0]["ok"], (s, r))
check("群里收到一条：列出 3 个没提交的单位和逾期说明", len(HOOKS) == 1 and all(k in json.dumps(HOOKS[0], ensure_ascii=False) for k in ("店铺A", "店铺B", "店铺C", "未指派", "已逾期 1 天")), HOOKS)
note = lambda tok: [n for n in inbox(tok) if n["kind"] == "collect_remind"]
check("Alice、Carol 收到催办通知（带逾期说明），来自催办的人", note(A) and note(C) and "店铺A" in note(A)[0]["text"] and "已逾期 1 天" in note(A)[0]["text"] and note(A)[0]["fromName"] == "Bob", (note(A), note(C)))
check("没有负责人的 C 没有人可通知；Bob 自己也没收到", note(Bo) == [], note(Bo))
s, r = remind(Bo); o = r["data"]
check("马上再催：1 小时内不重复（跳过 2 个，不再推送）", (o["notified"], o["skipped"], o["unassigned"]) == (0, 2, 1) and o["channels"] == [] and len(HOOKS) == 1 and len(note(A)) == 1, (r, len(HOOKS)))
d = call("GET", f"/ledger/collects/{CO}", R)[1]["data"]; by = {u["unit"]: u for u in d["units"]}
check("看板记下上次催办时间（C 没有负责人，没有记录）", by["店铺A"]["remindedAt"] and by["店铺B"]["remindedAt"] and by["店铺C"]["remindedAt"] is None, d["units"])
s, r = wb(A, DS, [row("店铺A", "2026-12", 5)]); check("A 提交了这一期", s == 200, (s, r))
s, r = call("GET", "/ledger/collects/mine", A); t = r["data"][0]; p = next(p for u in t["units"] for p in u["periods"] if p["period"] == "2026-12")
check("提交后不再逾期", not p["overdue"] and t["overdue"] == 0, t)
s, r = remind(R, "2026-12", ["店铺A"]); check("管理员催已经提交的单位：没有要催的", s == 400 and "都已提交" in msg((s, r)), (s, r))
s, r = remind(R, "2026-12", ["店铺C"]); o = r["data"] if s == 200 else r
check("管理员只催店铺 C：没有负责人，只在名单里（群里 1 小时内刚推过，不重复）", s == 200 and (o["notified"], o["unassigned"]) == (0, 1) and o["channels"] == [], (s, r))

# ── 用已有的数据集 ──
def new_dataset(name, pk):
    s, r = call("POST", "/ledger/datasets/input", R, body={"name": name, "columns": [{"title": "部门", "kind": "text"}, {"title": "月份", "kind": "text"}, {"title": "预算", "kind": "money"}], "primaryKey": pk})
    assert s == 200, (s, r)
    return r["data"]
existing = lambda ds, **kw: {"name": "预算收集", "datasetId": ds, "unitColumn": "部门", "periodColumn": "月份", "dimension": "部门", "units": [{"unit": "财务部"}], **kw}
weak = new_dataset("只按部门", ["部门"])
s, r = call("POST", "/ledger/collects", R, body=existing(weak)); check("已有数据集的主键不含期次列：被拒（会互相覆盖）", s == 400 and "主键必须包含" in msg((s, r)), (s, r))
good = new_dataset("部门预算", ["部门", "月份"])
s, r = call("POST", "/ledger/collects", R, body=existing(good, unitColumn="没有这列")); check("单位列不存在被拒", s == 400 and "没有「没有这列」列" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/collects", R, body=existing(good)); check("用已有数据集建任务（自定义列名和维度）", s == 200, (s, r))
CO2 = r["data"]
s, r = call("POST", "/ledger/collects", R, body=existing(good)); check("一个数据集只能属于一个任务", s == 400 and "已经属于另一个收集任务" in msg((s, r)), (s, r))
call("POST", f"/ledger/collects/{CO2}/periods", R, body={"period": "2026-10"})
s, r = wb(R, good, [["财务部", "2026-10", 9]], cols=["部门", "月份", "预算"]); check("自定义列名：提交后记为已提交", s == 200, (s, r))
s, r = call("GET", f"/ledger/collects/{CO2}", R); check("看板按自定义的列名统计", r["data"]["units"][0]["status"] == "submitted" and r["data"]["collect"]["unitColumn"] == "部门", r)

# ── 删除任务 ──
s, r = call("DELETE", f"/ledger/collects/{CO}", A); check("普通用户不能删任务", s == 403, (s, r))
s, r = call("DELETE", f"/ledger/collects/{CO}", R); check("管理员删除任务", s == 200, (s, r))
s, r = call("GET", "/ledger/collects", R); check("只剩另一个任务", [x["name"] for x in r["data"]] == ["预算收集"], r)
check("删除任务后数据集和已填的数据保留", cursor_total(R, DS) == 5, cursor_total(R, DS))
check("Alice 的范围收回，不再受限", cursor_total(A, DS) == 5, cursor_total(A, DS))
s, r = wb(R, DS, [["店铺X", 1]], cols=["填报单位", "营收"], mode="append"); check("数据集不再属于任何任务：提交不再要求期次列", s == 200 and r["data"]["inserted"] == 1, (s, r))
s, r = call("GET", f"/ledger/collects/{CO}", R); check("已删除的任务查不到", s == 400 and "不存在" in msg((s, r)), (s, r))

# ── 审计 ──
time.sleep(3)  # 审计按 2 秒一批异步写入
s, r = call("GET", "/ledger/audit?size=200", R)
actions = " ".join(x.get("action", "") for x in r["data"]["items"]) if s == 200 else str(r)
check("审计里有任务的新建、修改、删除，期次的开启、删除，填报工作簿的生成、审核、催办和截止日期", all(k in actions for k in ("新建收集任务", "修改收集任务", "删除收集任务", "开启收集期次", "删除收集期次", "生成填报工作簿", "审核填报", "催办收集填报", "设置收集期次截止日期")), actions[:300])

finish()
