"""透视表「计算项」：在行 / 列字段的取值之间做运算，新增一个取值（库存进销存的「期末 = 期初 + 入库 - 出库」）

对每个值、每个格子（明细、小计、总计）分别计算；总计列不含计算项；没有记录的兄弟取值按 0 算；
计算项排在同级取值最后；与计算值（值之间的运算）可以叠用。结果与 Python 独立计算对照。
"""
import random
from lib import *  # noqa: F403
for t in (A,): call("GET", "/ledger/me", t)
time.sleep(1)

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
def import_ds(tok, name, csv):
    s, r = call("POST", "/ledger/datasets/import/preview", tok, raw=mp(name + ".csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
    assert s == 200, (s, r)
    p = r["data"]
    s, r = call("POST", "/ledger/datasets/import/commit", tok, body={"token": p["token"], "mode": "create", "name": name, "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
    assert s == 200, (s, r)
    return r["data"]["datasetId"]

random.seed(11)
rows = []   # (商品, 月份, 单据类型, 数量, 金额)
for p in ["P1", "P2", "P3", "P4"]:
    for mth in ["2026-01", "2026-02", "2026-03"]:
        for kind in ["期初", "入库", "出库"]:
            if p == "P4" and kind == "出库":
                continue       # P4 从来没有出库记录：期末 = 期初 + 入库
            for _ in range(random.randint(1, 4)):
                rows.append((p, mth, kind, random.randint(1, 50), round(random.uniform(10, 500), 2)))
csv = "商品,月份,单据类型,数量,金额\n" + "\n".join(",".join(map(str, r)) for r in rows) + "\n"
DS = import_ds(A, "进销存测试", csv)
pivot = lambda body: call("POST", f"/ledger/datasets/{DS}/pivot", A, body=body)
qty = lambda f: sum(r[3] for r in rows if f(r))
amt = lambda f: round(sum(r[4] for r in rows if f(r)), 2)
close = lambda a, b: a is not None and b is not None and abs(a - b) < 0.006

END = {"field": "单据类型", "name": "期末", "formula": "[期初] + [入库] - [出库]"}

# ── 1. 列字段上的计算项（库存进销存）──
s, r = pivot({"rows": [{"column": "商品"}], "columns": [{"column": "单据类型"}], "values": [{"column": "数量", "agg": "sum"}], "calcItems": [END]})
check("请求成功", s == 200, (s, r))
res = r["data"]
keys = [k[0] for k in res["columnKeys"]]
check("计算项排在同级取值最后", keys == ["入库", "出库", "期初", "期末"], keys)
data = {x["keys"][0]: x["cells"] for x in res["rows"] if x["kind"] == "data"}
ok = True
for p in ["P1", "P2", "P3", "P4"]:
    want = qty(lambda r_, p_=p: r_[0] == p_ and r_[2] == "期初") + qty(lambda r_, p_=p: r_[0] == p_ and r_[2] == "入库") - qty(lambda r_, p_=p: r_[0] == p_ and r_[2] == "出库")
    ok &= data[p][3][0] == want
check("每个商品：期末 = 期初 + 入库 - 出库", ok, data)
p4 = data["P4"]
check("P4 没有出库记录（出库为空）：期末按 0 算 = 期初 + 入库", p4[1][0] is None and p4[3][0] == qty(lambda r_: r_[0] == "P4" and r_[2] == "期初") + qty(lambda r_: r_[0] == "P4" and r_[2] == "入库"), p4)
total_row = [x for x in res["rows"] if x["kind"] == "total"][0]["cells"]
want_total = qty(lambda r_: r_[2] == "期初") + qty(lambda r_: r_[2] == "入库") - qty(lambda r_: r_[2] == "出库")
check("总计行的期末 = 各类型总计的组合", total_row[3][0] == want_total, (total_row, want_total))
row_total_col = data["P1"][-1][0]
check("总计列（跨列合计）不含计算项：= 期初 + 入库 + 出库", row_total_col == qty(lambda r_: r_[0] == "P1"), (row_total_col, qty(lambda r_: r_[0] == "P1")))
check("总计列的数量与列数：4 个取值 + 1 个总计", len(data["P1"]) == 5)

# ── 2. 多个值：每个值分别算 ──
s, r = pivot({"rows": [{"column": "商品"}], "columns": [{"column": "单据类型"}], "values": [{"column": "数量", "agg": "sum"}, {"column": "金额", "agg": "sum"}], "calcItems": [END]})
res = r["data"]
d1 = [x for x in res["rows"] if x["kind"] == "data" and x["keys"][0] == "P2"][0]["cells"]
want_amt = amt(lambda r_: r_[0] == "P2" and r_[2] == "期初") + amt(lambda r_: r_[0] == "P2" and r_[2] == "入库") - amt(lambda r_: r_[0] == "P2" and r_[2] == "出库")
check("金额也按公式算（保留到分）", close(d1[3][1], want_amt), (d1[3], want_amt))

# ── 3. 与计算值叠用：期末那一列的「单价」按期末的金额 / 数量算 ──
s, r = pivot({"rows": [{"column": "商品"}], "columns": [{"column": "单据类型"}], "values": [{"column": "数量", "agg": "sum"}, {"column": "金额", "agg": "sum"}, {"agg": "calc", "label": "单价", "formula": "[金额（求和）] / [数量（求和）]", "format": "money"}], "calcItems": [END]})
res = r["data"]
d1 = [x for x in res["rows"] if x["kind"] == "data" and x["keys"][0] == "P2"][0]["cells"]
check("计算项那一列上，计算值按计算项的汇总算", s == 200 and close(d1[3][2], d1[3][1] / d1[3][0]), d1[3])

# ── 4. 行字段上的计算项（两级行：每个商品下多一行「环差」，小计不含）──
DIFF = {"field": "月份", "name": "环差", "formula": "[2026-03] - [2026-02]"}
s, r = pivot({"rows": [{"column": "商品"}, {"column": "月份"}], "values": [{"column": "数量", "agg": "sum"}], "calcItems": [DIFF]})
check("行字段计算项请求成功", s == 200, (s, r))
res = r["data"]
leaf = {(x["keys"][0], x["keys"][1]): x["cells"][-1][0] for x in res["rows"] if x["kind"] == "data"}
ok = all(leaf[(p, "环差")] == qty(lambda r_, p_=p: r_[0] == p_ and r_[1] == "2026-03") - qty(lambda r_, p_=p: r_[0] == p_ and r_[1] == "2026-02") for p in ["P1", "P2", "P3", "P4"])
check("每个商品下多一行「环差」，值正确", ok, leaf)
order_p1 = [x["keys"][1] for x in res["rows"] if x["kind"] == "data" and x["keys"][0] == "P1"]
check("「环差」排在该商品的月份之后", order_p1 == ["2026-01", "2026-02", "2026-03", "环差"], order_p1)
sub = {x["keys"][0]: x["cells"][-1][0] for x in res["rows"] if x["kind"] == "subtotal"}
check("商品小计不含计算项（= 该商品全部数量）", all(sub[p] == qty(lambda r_, p_=p: r_[0] == p_) for p in sub), sub)
tot = [x for x in res["rows"] if x["kind"] == "total"][0]["cells"][-1][0]
check("总计不含计算项", tot == qty(lambda r_: True), tot)
# 倒序时计算项仍在最后
s, r = pivot({"rows": [{"column": "月份"}], "values": [{"column": "数量", "agg": "sum"}], "calcItems": [DIFF], "sort": {"by": "key", "desc": True}})
order = [x["keys"][0] for x in r["data"]["rows"] if x["kind"] == "data"]
check("倒序排列时计算项仍排在最后", order == ["2026-03", "2026-02", "2026-01", "环差"], order)

# ── 5. 行 + 列各一个计算项：交叉处也算 ──
s, r = pivot({"rows": [{"column": "月份"}], "columns": [{"column": "单据类型"}], "values": [{"column": "数量", "agg": "sum"}], "calcItems": [END, {"field": "月份", "name": "环差", "formula": "[2026-03] - [2026-02]"}]})
res = r["data"]
cross = [x for x in res["rows"] if x["kind"] == "data" and x["keys"][0] == "环差"][0]["cells"]
keys = [k[0] for k in res["columnKeys"]]
want = (qty(lambda r_: r_[1] == "2026-03" and r_[2] == "期初") + qty(lambda r_: r_[1] == "2026-03" and r_[2] == "入库") - qty(lambda r_: r_[1] == "2026-03" and r_[2] == "出库")) - (qty(lambda r_: r_[1] == "2026-02" and r_[2] == "期初") + qty(lambda r_: r_[1] == "2026-02" and r_[2] == "入库") - qty(lambda r_: r_[1] == "2026-02" and r_[2] == "出库"))
check("行、列各有计算项：交叉格 = 两个计算项的组合", s == 200 and cross[keys.index("期末")][0] == want, (cross, keys, want))

# ── 6. 写错时的提示 ──
def bad(name, body, contains):
    s, r = pivot(body)
    check(name, s == 400 and contains in (r.get("msg") or ""), (s, r.get("msg")))
base = {"rows": [{"column": "商品"}], "columns": [{"column": "单据类型"}], "values": [{"column": "数量", "agg": "sum"}]}
bad("引用不存在的取值：列出现有取值", {**base, "calcItems": [{"field": "单据类型", "name": "期末", "formula": "[期初] + [报废]"}]}, "引用的「报废」不是「单据类型」里的取值（现有：[入库]、[出库]、[期初]）")
bad("名称与已有取值重名", {**base, "calcItems": [{"field": "单据类型", "name": "入库", "formula": "[期初] + 1"}]}, "已有的取值重名")
bad("字段不在行或列里", {**base, "calcItems": [{"field": "月份", "name": "x", "formula": "[2026-01] + 1"}]}, "不在行或列里")
bad("公式里没有引用任何取值", {**base, "calcItems": [{"field": "单据类型", "name": "常数", "formula": "1 + 1"}]}, "没有引用任何取值")
bad("计算项不能引用另一个计算项", {**base, "calcItems": [END, {"field": "单据类型", "name": "再算", "formula": "[期末] * 2"}]}, "引用了另一个计算项")
bad("有计算项时值不能设占比", {**base, "values": [{"column": "数量", "agg": "sum", "show": "pct_total"}], "calcItems": [END]}, "不能设置占比")
bad("有计算项时不能用前 N 项", {**base, "top": 2, "calcItems": [END]}, "前 N 项")
bad("公式语法错误带上计算项名称", {**base, "calcItems": [{"field": "单据类型", "name": "期末", "formula": "([期初]"}]}, "计算项「期末」")
bad("两个同名计算项", {**base, "calcItems": [END, END]}, "同名的计算项")

# ── 7. 没有计算项时照旧 ──
s, r = pivot(base)
check("没有计算项：列键与以前一致，没有「期末」", s == 200 and [k[0] for k in r["data"]["columnKeys"]] == ["入库", "出库", "期初"], r["data"]["columnKeys"] if s == 200 else r)
finish()
