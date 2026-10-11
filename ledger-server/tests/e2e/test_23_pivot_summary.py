"""透视表「转为数据表」：汇总数据集（按行展平的透视结果，随源数据实时变化）

校验（与 Python 独立计算对照）：分组与汇总值、计算值（除数为 0 为空）、源数据的筛选与日期分组；
按汇总值筛选 / 排序（结果再筛选）、在它上面加计算列、再做透视、写 SQL；源数据追加后自动更新；
受限用户只汇总自己可见的行；不能填报写入、源数据集被引用时不能删除、删除进回收站可恢复；各种写错的说明。
"""
from collections import defaultdict
from lib import *  # noqa: F403
for t in (A, Bo, R): call("GET", "/ledger/me", t)
time.sleep(1)

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
def upload(tok, name, csv, mode="create", ds=None):
    s, r = call("POST", "/ledger/datasets/import/preview", tok, raw=mp(name + ".csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
    assert s == 200, (s, r)
    p = r["data"]
    body = {"token": p["token"], "mode": mode, "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]}
    if mode == "create": body["name"] = name
    else: body["datasetId"] = ds
    s, r = call("POST", "/ledger/datasets/import/commit", tok, body=body)
    assert s == 200, (s, r)
    return r["data"]["datasetId"]

ROWS = [
    ("A", "抖音", "2026-09-03", 100, 60), ("A", "抖音", "2026-10-05", 300, 100), ("A", "淘宝", "2026-10-06", 50, 50),
    ("B", "抖音", "2026-09-10", 1000, 400), ("B", "淘宝", "2026-10-01", 2000, 900),
    ("C", "淘宝", "2026-10-02", 0, 30), ("D", "抖音", "2026-10-09", 5, 1),
]
csv = lambda rs: "店铺,渠道,下单时间,金额,成本\n" + "\n".join(",".join(map(str, r)) for r in rs) + "\n"
ORD = upload(A, "订单", csv(ROWS))

def expect(rows, key=lambda r: (r[0], r[1]), keep=lambda r: True):
    g = defaultdict(lambda: [0, 0, 0])
    for r in rows:
        if keep(r):
            k = key(r); g[k][0] += r[3]; g[k][1] += r[4]; g[k][2] += 1
    return {k: (a, c, n, (a - c) / a if a else None) for k, (a, c, n) in g.items()}

VALUES = [
    {"column": "金额", "agg": "sum"}, {"column": "成本", "agg": "sum"}, {"agg": "count"},
    {"agg": "calc", "label": "毛利率", "formula": "([金额（求和）] - [成本（求和）]) / [金额（求和）]", "format": "percent"},
]
def create(name, rows, values=VALUES, tok=A, **extra):
    return call("POST", "/ledger/datasets/summaries", tok, body={"name": name, "def": {"source": ORD, "rows": rows, "values": values, **extra}})
def ds_list(tok=A):
    s, r = call("GET", "/ledger/datasets", tok)
    return {d["id"]: d for d in r["data"]}
def all_rows(ds, tok=A, **spec):
    s, r = call("POST", f"/ledger/datasets/{ds}/cursor", tok, body={"filters": [], "keyword": None, "sort": [], "rule": None, **spec})
    assert s == 200, (s, r)
    s, rr = call("GET", f"/ledger/datasets/{ds}/rows?cursor={r['data']['cursorId']}&offset=0&limit=500", tok)
    assert s == 200, (s, rr)
    return rr["data"]["rows"]
def close(a, b):
    return (a is None and b is None) or (a is not None and b is not None and abs(a - b) < 1e-9)

# ── 新建 ──
s, r = create("店铺渠道汇总", [{"column": "店铺"}, {"column": "渠道"}])
check("新建汇总数据集", s == 200, (s, r))
SM = r["data"]
d = ds_list().get(SM, {})
cols = [(c["title"], c["kind"]) for c in d.get("columns", [])]
check("列：分组列 + 汇总值 + 计算值，类型对", cols == [("店铺", "text"), ("渠道", "text"), ("金额（求和）", "money"), ("成本（求和）", "money"), ("计数", "number"), ("毛利率", "number")], cols)
check("类型为 summary、带源数据集", d.get("kind") == "summary" and d.get("summary", {}).get("source") == ORD, d.get("kind"))
want = expect(ROWS)
got = {(x[0], x[1]): tuple(x[2:6]) for x in all_rows(SM)}
check("行数 = 分组数", d.get("rowCount") == len(want) and len(got) == len(want), (d.get("rowCount"), len(want)))
check("每组的金额、成本、计数与独立计算一致", all(got[k][:3] == want[k][:3] for k in want), (got, want))
check("毛利率逐组按公式计算，C 店金额为 0 时为空", all(close(got[k][3], want[k][3]) for k in want) and got[("C", "淘宝")][3] is None, got)

# ── 结果再筛选 / 排序 ──
rows = all_rows(SM, filters=[{"op": "between", "column": "金额（求和）", "min": 300}], sort=[{"column": "金额（求和）", "desc": True}])
check("按汇总值筛选（金额合计 ≥ 300）并降序", [(x[0], x[1]) for x in rows] == [("B", "淘宝"), ("B", "抖音"), ("A", "抖音")], rows)
rows = all_rows(SM, filters=[{"op": "in", "column": "渠道", "values": ["淘宝"]}])
check("按分组列筛选", sorted(x[0] for x in rows) == ["A", "B", "C"], rows)

# ── 在汇总上加计算列、再做透视、写 SQL ──
s, r = call("POST", f"/ledger/datasets/{SM}/calc", A, body={"name": "毛利", "formula": "=[金额（求和）]-[成本（求和）]"})
check("在汇总数据集上加计算列", s == 200, (s, r))
rows = all_rows(SM, filters=[{"op": "in", "column": "店铺", "values": ["B"]}])
check("计算列可用：B 店两组毛利 600、1100", sorted(x[-1] for x in rows) == [600, 1100], rows)
s, r = call("POST", f"/ledger/datasets/{SM}/pivot", A, body={"rows": [{"column": "店铺"}], "values": [{"column": "金额（求和）", "agg": "sum"}, {"column": "毛利", "agg": "sum"}]})
pv = {x["keys"][0]: x["cells"][-1] for x in r["data"]["rows"] if x["kind"] == "data"} if s == 200 else {}
check("在汇总数据集上再透视：按店铺合计与原始数据一致", pv.get("A") == [450, 240] and pv.get("B") == [3000, 1700], (s, pv))
s, r = call("POST", "/ledger/sql/run", A, body={"sql": 'SELECT sum("金额（求和）") AS 合计, count(*) AS 组数 FROM 店铺渠道汇总'})
check("SQL 里按名称查询", s == 200 and r["data"]["rows"][0] == [sum(x[3] for x in ROWS), len(want)], (s, r))

# ── 源数据变化后自动更新 ──
more = [("D", "淘宝", "2026-10-10", 70, 20), ("A", "抖音", "2026-10-11", 1, 1)]
upload(A, "订单", csv(more), mode="append", ds=ORD)
got = {(x[0], x[1]): tuple(x[2:5]) for x in all_rows(SM)}
want2 = expect(ROWS + more)
check("源数据追加后汇总自动更新（新组 D/淘宝、A/抖音 金额 401）", got.get(("D", "淘宝")) == want2[("D", "淘宝")][:3] and got.get(("A", "抖音"))[0] == 401, got)

# ── 日期分组、源数据筛选、首个 / 末个 ──
s, r = create("月度汇总", [{"column": "下单时间", "granularity": "month"}, {"column": "店铺"}], values=[{"column": "金额", "agg": "sum"}, {"column": "金额", "agg": "last"}], filters=[{"op": "in", "column": "渠道", "values": ["抖音"]}])
check("按月分组 + 只汇总抖音渠道", s == 200, (s, r))
MO = r["data"]
cols = [(c["title"], c["kind"]) for c in ds_list()[MO]["columns"]]
check("日期分组列为文本，表头带（月）", cols[0] == ("下单时间（月）", "text"), cols)
got = {(x[0], x[1]): (x[2], x[3]) for x in all_rows(MO)}
want3 = expect(ROWS + more, key=lambda r: (r[2][:7], r[0]), keep=lambda r: r[1] == "抖音")
check("每月每店的抖音金额与独立计算一致", {k: v[0] for k, v in got.items()} == {k: v[0] for k, v in want3.items()}, (got, want3))
check("末个：A 店 10 月最后一笔抖音订单金额 1", got.get(("2026-10", "A"), (None, None))[1] == 1, got)

# ── 写错的情形 ──
s, r = create("x1", [{"column": "店铺"}], values=[{"column": "金额", "agg": "sum", "show": "pct_total"}]); check("占比不能带入", s == 400 and "占比" in msg((s, r)), (s, r))
s, r = create("x2", [], values=[{"column": "金额", "agg": "sum"}]); check("至少一个分组字段", s == 400 and "分组" in msg((s, r)), (s, r))
s, r = create("x3", [{"column": "店铺"}], values=[{"column": "金额", "agg": "sum", "label": "店铺"}]); check("列名重复给出说明", s == 400 and "重复" in msg((s, r)), (s, r))
s, r = create("x4", [{"column": "门店"}]); check("列不存在", s == 400 and "门店" in msg((s, r)), (s, r))
s, r = create("店铺渠道汇总", [{"column": "店铺"}]); check("重名", s == 400 and "已存在" in msg((s, r)), (s, r))
s, r = create("x5", [{"column": "店铺"}], values=[{"agg": "calc", "label": "y", "formula": "[不存在]"}, {"column": "金额", "agg": "sum"}]); check("计算值引用不存在的值", s == 400 and "不是透视表里的值" in msg((s, r)), (s, r))
s, r = call("POST", f"/ledger/datasets/{SM}/writeback", A, body={"mode": "append", "columns": ["店铺"], "rows": [["Z"]]})
check("不能填报写入", s == 400 and "汇总数据集" in msg((s, r)), (s, r))

# ── 删除与回收站 ──
s, r = call("DELETE", f"/ledger/datasets/{ORD}?operator=Alice", A)
check("源数据集被汇总引用时不能删除", s == 400 and "店铺渠道汇总" in msg((s, r)), (s, r))
s, r = call("DELETE", f"/ledger/datasets/{MO}?operator=Alice", A); check("删除汇总数据集", s == 200, (s, r))
check("删除后不在列表里", MO not in ds_list())
s, r = call("GET", "/ledger/datasets/trash", A)
item = next((i for i in r["data"]["items"] if i["id"] == MO), None)
check("进回收站", item is not None, r["data"])
s, r = call("POST", f"/ledger/datasets/{MO}/untrash", A); check("恢复", s == 200 and MO in ds_list(), (s, r))

# ── 受限用户 ──
s, r = call("GET", "/ledger/users?keyword=", R)
ids = {u["name"]: u["id"] for u in r["data"]}
s, r = call("PUT", f"/ledger/data-scope/datasets/{ORD}", R, body={"scopeColumn": "店铺", "open": False}); check("订单按店铺列划分范围", s == 200, (s, r))
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["A"]}); check("Bob 限定 A 店", s == 200, (s, r))
bob = ds_list(Bo)
check("Bob 能看到汇总数据集", SM in bob, list(bob))
rows = all_rows(SM, tok=Bo)
check("Bob 看到的汇总只有 A 店的组，金额是 A 店自己的", sorted({x[0] for x in rows}) == ["A"] and {x[1]: x[2] for x in rows} == {"抖音": 401, "淘宝": 50}, rows)
check("Bob 的行数 = A 店的组数", bob[SM]["rowCount"] == 2, bob[SM])
s, r = call("POST", "/ledger/sql/run", Bo, body={"sql": 'SELECT sum("金额（求和）") FROM 店铺渠道汇总'})
check("Bob 用 SQL 查汇总也只有 A 店", s == 200 and r["data"]["rows"][0][0] == 451, (s, r))
s, r = create("Bob 的汇总", [{"column": "店铺"}], tok=Bo); check("受限用户不能新建汇总数据集（403）", s == 403, (s, r))
check("Alice 不受影响：仍是全部的组", len(all_rows(SM)) == len(want2), len(all_rows(SM)))
s, r = call("PUT", f"/ledger/data-scope/datasets/{ORD}", R, body={"scopeColumn": None, "open": False})
check("订单取消划分（对受限用户不可见）后，Bob 也看不到汇总", SM not in ds_list(Bo), list(ds_list(Bo)))
finish()
