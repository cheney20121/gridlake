"""填报：按人限制可填范围 + 按主键删除行

受限用户（Bob 只能看 A、B 店）只能填报「按店铺划分范围」的数据集，且只能填 / 改 / 删自己范围内的行；
删除按主键，可撤销（原样插回）。无范围列、所有人可见、未绑定的数据集对受限用户不开放。
"""
from lib import *  # noqa: F403
for t in (A, Bo, C, R): call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}

s, r = call("POST", "/ledger/datasets/input", A, body={"name": "分店预算", "columns": [{"title": "店铺", "kind": "text"}, {"title": "月份", "kind": "text"}, {"title": "预算", "kind": "money"}], "primaryKey": ["月份"]})
assert s == 200, (s, r)
BUD = r["data"]
s, r = call("POST", "/ledger/datasets/input", A, body={"name": "共享预算", "columns": [{"title": "店铺", "kind": "text"}, {"title": "预算", "kind": "money"}], "primaryKey": ["店铺"]})
SHARED = r["data"]
s, r = call("POST", "/ledger/datasets/input", A, body={"name": "未绑定", "columns": [{"title": "店铺", "kind": "text"}, {"title": "预算", "kind": "money"}], "primaryKey": ["店铺"]})
HIDDEN = r["data"]
call("PUT", f"/ledger/data-scope/datasets/{BUD}", R, body={"scopeColumn": "店铺", "open": False})
call("PUT", f"/ledger/data-scope/datasets/{SHARED}", R, body={"open": True})
call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["A", "B"]})

wb = lambda tok, ds, mode, cols, rows, **kw: call("POST", f"/ledger/datasets/{ds}/writeback", tok, body={"mode": mode, "columns": cols, "rows": rows, **kw})
COLS = ["店铺", "月份", "预算"]

# 管理员（不受限）先录两行：A 的 2026-11、C 的 2026-12
s, r = wb(A, BUD, "upsert", COLS, [["A", "2026-11", 10], ["C", "2026-12", 30]]); check("不受限用户录入", s == 200 and r["data"]["inserted"] == 2, (s, r))

# ── Bob：范围内可以填 ──
s, r = wb(Bo, BUD, "upsert", COLS, [["A", "2026-11", 11], ["B", "2027-01", 20]]); d = r["data"] if s == 200 else r
check("Bob 更新自己范围内的行、新增 B 店的行", s == 200 and d["inserted"] == 1 and d["updated"] == 1 and d["batchId"], (s, r))
s, r = wb(Bo, BUD, "upsert", COLS, [["C", "2027-02", 1], ["A", "2027-03", 5]]); d = r["data"] if s == 200 else r
check("范围外的店铺：整批不写入，逐格说明", s == 200 and d["batchId"] is None and d["errorCount"] == 1 and "不在你的数据范围内" in d["errors"][0]["message"] and d["errors"][0]["row"] == 0, (s, r))
s, r = wb(Bo, BUD, "upsert", COLS, [[None, "2027-02", 1]]); check("范围列为空被拒", s == 200 and r["data"]["errorCount"] == 1 and "不能为空" in r["data"]["errors"][0]["message"], (s, r))
s, r = wb(Bo, BUD, "append", ["月份", "预算"], [["2027-04", 1]]); check("填报区没有范围列直接拒绝并说明", s == 400 and "必须包含「店铺」" in msg((s, r)), (s, r))
s, r = wb(Bo, BUD, "upsert", COLS, [["A", "2026-12", 99]]); d = r["data"] if s == 200 else r
check("主键撞上别人范围内的行（C 店的 2026-12）：不能改", s == 200 and d["batchId"] is None and "不在你的数据范围内" in d["errors"][0]["message"], (s, r))
s, r = wb(Bo, BUD, "delete", ["月份"], [["2026-12"]]); d = r["data"] if s == 200 else r
check("也不能删别人范围内的行", s == 200 and d["batchId"] is None and d["errorCount"] == 1, (s, r))
s, r = call("POST", f"/ledger/datasets/{BUD}/aggregate", A, body={"calls": [{"func": "sum", "column": "预算", "criteria": [{"column": "店铺", "value": "C"}]}]})
check("C 店的数据没被动过", r["data"]["values"][0] == 30, r)

# ── 删除 ──
s, r = wb(Bo, BUD, "delete", ["月份"], [["2027-01"], ["2099-01"]]); d = r["data"]
check("Bob 删自己范围内的行；不存在的主键只计数", s == 200 and d["deleted"] == 1 and d["notFound"] == 1 and d["inserted"] == 0, (s, r))
BATCH = d["batchId"]
s, r = call("POST", f"/ledger/datasets/{BUD}/aggregate", A, body={"calls": [{"func": "count", "criteria": []}]}); check("数据集里少了一行（共 2 行）", r["data"]["values"][0] == 2, r)
s, r = wb(A, BUD, "delete", ["月份", "预算"], [["2026-11", "不检查"], ["2026-11", 1]]); d = r["data"]
check("删除时主键重复被拒（整批不删）", d["batchId"] is None and d["errorCount"] == 1, d)
s, r = wb(A, BUD, "delete", ["预算"], [[1]]); check("删除的填报区必须含主键列", s == 400 and "主键" in msg((s, r)), (s, r))
# 撤销：受限用户不行（需要完整权限），不受限用户可以
s, r = call("POST", f"/ledger/writebacks/{BATCH}/undo", Bo); check("受限用户不能撤销", s == 403, (s, r))
s, r = call("GET", f"/ledger/writebacks?datasetId={BUD}", A); rec = next(x for x in r["data"] if x["id"] == BATCH)
check("提交记录有「删除」数且可撤销", rec["mode"] == "delete" and rec["deleted"] == 1 and rec["canUndo"] and rec["operator"] == "Bob", rec)
s, r = call("POST", f"/ledger/writebacks/{BATCH}/undo", A); check("撤销把删除的行插回", s == 200 and r["data"]["reinserted"] == 1, (s, r))
s, r = call("POST", f"/ledger/datasets/{BUD}/aggregate", A, body={"calls": [{"func": "count", "criteria": []}, {"func": "sum", "column": "预算", "criteria": [{"column": "店铺", "value": "B"}]}]})
check("回到 3 行，B 店 20", r["data"]["values"] == [3, 20], r)
s, r = wb(Bo, BUD, "append", COLS, [["B", "2027-05", 7]]); check("Bob 追加自己范围内的行", s == 200 and r["data"]["inserted"] == 1, (s, r))

# 受限用户看到的数据仍然只有自己的范围
spec = {"filters": [], "keyword": None, "sort": [], "rule": None}
s, r = call("POST", f"/ledger/datasets/{BUD}/cursor", Bo, body=spec); check("Bob 只看到 A、B 的行（3 行，C 店的看不到）", r["data"]["total"] == 3, r)

# ── 其他数据集 ──
s, r = wb(Bo, SHARED, "upsert", ["店铺", "预算"], [["A", 1]]); check("所有人可见的共享数据集：受限用户不能填", s == 403 and "只能填报按店铺划分范围" in msg((s, r)), (s, r))
s, r = wb(Bo, HIDDEN, "upsert", ["店铺", "预算"], [["A", 1]]); check("没有绑定的数据集：对受限用户不存在", s == 400 and "不存在" in msg((s, r)), (s, r))
s, r = wb(A, SHARED, "upsert", ["店铺", "预算"], [["A", 1]]); check("不受限用户照常可以填共享数据集", s == 200 and r["data"]["inserted"] == 1, (s, r))
s, r = call("POST", "/ledger/datasets/input", Bo, body={"name": "x", "columns": [], "primaryKey": []}); check("受限用户不能新建填报数据集", s == 403, (s, r))
# Carol 没有规则，不受限
s, r = wb(C, BUD, "upsert", COLS, [["C", "2026-12", 31]]); check("没有数据范围规则的用户可以改任何行", s == 200 and r["data"]["updated"] == 1, (s, r))
# 审计：谁提交了填报（受限用户的也记）
time.sleep(3)  # 审计异步写入
s, r = call("GET", "/ledger/audit?size=200&user=Bob", R)
acts = [x["action"] for x in r["data"]["items"]]
check("操作审计里有 Bob 的填报提交", s == 200 and acts.count("填报提交") >= 4, acts)
finish()
