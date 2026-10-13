"""多人填报：收集任务（任务与期次、负责人的数据范围、提交校验与状态看板、我的填报、权限）

场景：四家店铺每月报经营数据。店铺 A、D 归 Bob，店铺 B 归 Carol，店铺 C 没有指派（管理员代填）。
所有人往同一个数据集里填，各填各的行；看板按单位 × 期次显示谁填了。
"""
from lib import *  # noqa: F403

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
check("删除任务后数据集和已填的数据保留", cursor_total(R, DS) == 4, cursor_total(R, DS))
check("Alice 的范围收回，不再受限", cursor_total(A, DS) == 4, cursor_total(A, DS))
s, r = wb(R, DS, [["店铺X", 1]], cols=["填报单位", "营收"], mode="append"); check("数据集不再属于任何任务：提交不再要求期次列", s == 200 and r["data"]["inserted"] == 1, (s, r))
s, r = call("GET", f"/ledger/collects/{CO}", R); check("已删除的任务查不到", s == 400 and "不存在" in msg((s, r)), (s, r))

# ── 审计 ──
time.sleep(3)  # 审计按 2 秒一批异步写入
s, r = call("GET", "/ledger/audit?size=200", R)
actions = " ".join(x.get("action", "") for x in r["data"]["items"]) if s == 200 else str(r)
check("审计里有任务的新建、修改、删除和期次的开启、删除", all(k in actions for k in ("新建收集任务", "修改收集任务", "删除收集任务", "开启收集期次", "删除收集期次")), actions[:300])

finish()
