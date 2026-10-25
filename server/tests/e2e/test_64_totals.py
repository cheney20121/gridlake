"""汇总行：按当前筛选（游标）对整个结果集算每列的合计 / 平均 / 中位数 / 最小 / 最大 / 计数 / 空值 / 不同值 / 「是」的个数；
金额保留两位、整数精确；日期列取最早 / 最晚并跳过认不出的值；空白文本算空值；列类型不支持的汇总方式拒绝；
游标失效时提示重新查询；数据范围受限的用户只汇总自己能看到的行（拿别人的游标也一样）；下钻的贡献金额列可以汇总。
"""
from lib import *  # noqa: F403

for t in (A, Bo, R):
    call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()

KINDS = {"金额": "money", "数量": "integer", "日期": "datetime", "已付": "boolean", "折扣": "number"}
s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("汇总订单.csv", (
    "店铺,金额,数量,日期,已付,折扣,备注\n"
    "北京店,10.005,3,2026-03-01,是,0.5,加急\n"
    "北京店,20,9007199254740993,2026-01-15 10:00,否,,\n"
    "上海店,30.5,2,2026-02-01,是,0.25,加急\n"
    "上海店,,1,下周一,是,1,普通\n"
    "广州店,-60.5,4,,否,0.25,\n")), ctype=f"multipart/form-data; boundary={boundary}")
assert s == 200, (s, r)
p = r["data"]
s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": "create", "name": "汇总订单", "columns": [{"source": c["source"], "title": c["title"], "kind": KINDS.get(c["title"], "text")} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
assert s == 200, (s, r)
D = r["data"]["datasetId"]

spec = lambda **k: {"filters": [], "keyword": None, "sort": [], "rule": None, **k}
def cursor(tok=A, **k):
    s, r = call("POST", f"/ledger/datasets/{D}/cursor", tok, body=spec(**k))
    assert s == 200, (s, r)
    return r["data"]
def totals(cur, items, tok=A):
    return call("POST", f"/ledger/datasets/{D}/totals", tok, body={"cursorId": cur["cursorId"], "items": [{"column": c, "func": f} for c, f in items]})

c = cursor()
s, r = totals(c, [("金额", "sum"), ("金额", "average"), ("金额", "median"), ("金额", "min"), ("金额", "max"), ("金额", "count"), ("金额", "empty")])
check("金额：合计 / 平均 / 中位数 / 最小 / 最大 / 计数 / 空值（金额保留两位）", s == 200 and r["data"]["values"] == [0.01, 0.0, 15.0, -60.5, 30.5, 4, 1], (s, r))
s, r = totals(c, [("数量", "sum"), ("折扣", "average"), ("折扣", "distinct")])
check("整数合计精确（超出 JS 安全整数时按文本返回，不丢精度），数值列平均、不同值", s == 200 and r["data"]["values"] == ["9007199254741003", 0.5, 3], (s, r))
s, r = totals(c, [("日期", "min"), ("日期", "max"), ("日期", "count"), ("日期", "empty")])
check("日期：最早 / 最晚跳过认不出的「下周一」；空日期算空值", s == 200 and r["data"]["values"] == ["2026-01-15 10:00:00", "2026-03-01 00:00:00", 4, 1], (s, r))
s, r = totals(c, [("备注", "count"), ("备注", "empty"), ("备注", "distinct"), ("店铺", "distinct"), ("已付", "trues")])
check("文本：空白算空值；不同值不含空；「是」的个数", s == 200 and r["data"]["values"] == [3, 2, 2, 3, 3], (s, r))

# ── 按当前筛选 ──
cf = cursor(filters=[{"op": "in", "column": "店铺", "values": ["北京店", "上海店"]}])
s, r = totals(cf, [("金额", "sum"), ("店铺", "distinct")])
check("按筛选后的结果集汇总", s == 200 and r["data"]["values"] == [60.51, 2], (s, r))
ck = cursor(keyword="加急")
s, r = totals(ck, [("金额", "sum")])
check("关键字搜索也算在内", s == 200 and r["data"]["values"] == [40.51], (s, r))
s, r = totals(cursor(filters=[{"op": "in", "column": "店铺", "values": ["不存在"]}]), [("金额", "sum"), ("金额", "count")])
check("没有符合条件的行：合计为空、计数为 0", s == 200 and r["data"]["values"] == [None, 0], (s, r))

# ── 拒绝 ──
s, r = totals(c, [("店铺", "sum")])
check("文本列不能求和，说明原因", s == 400 and "店铺" in msg((s, r)) and "求和" in msg((s, r)), (s, r))
s, r = totals(c, [("已付", "average")])
check("是/否列不能求平均", s == 400, (s, r))
s, r = totals(c, [("不存在的列", "count")])
check("列不存在", s >= 400, (s, r))
s, r = totals(c, [("__contribution", "sum")])
check("没有规则时没有贡献金额列", s == 400, (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/totals", A, body={"cursorId": "nope", "items": [{"column": "金额", "func": "sum"}]})
check("游标失效：返回 expired，前端重新查询", s == 200 and r["data"]["expired"], (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/totals", A, body={"cursorId": c["cursorId"], "items": [{"column": "金额", "func": "sum"}] * 501})
check("一次最多 500 项", s == 400, (s, r))
s, r = totals(c, [("金额", "sum")], tok=None)
check("未登录不能用", s == 401, (s, r))

# ── 数据范围 ──
call("PUT", f"/ledger/data-scope/datasets/{D}", R, body={"scopeColumn": "店铺", "open": False})
call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["北京店"]})
cb = cursor(Bo)
s, r = totals(cb, [("金额", "sum"), ("店铺", "distinct")], tok=Bo)
check("数据范围受限：只汇总自己店铺的行", s == 200 and r["data"]["values"] == [30.01, 1], (s, r))
ca = cursor()
s, r = totals(ca, [("金额", "sum")], tok=Bo)
check("拿别人的游标也只汇总自己能看到的行", s == 200 and r["data"]["values"] in ([30.01], [None]) or (s == 200 and r["data"]["expired"]), (s, r))
call("DELETE", f"/ledger/data-scope/rules/{ids['Bob']}", R)
finish()
