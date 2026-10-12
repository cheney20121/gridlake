"""透视表「计算值」：用公式把已汇总的值组合成新值（毛利、毛利率、客单价）

公式在汇总之后按每个格子单独算（明细、小计、总计、每一列）：总计行的毛利率是「总毛利 / 总金额」，不是各行毛利率的平均。
结果与 Python 独立计算对照；空值 / 除数为 0 得到空；各种写错公式的情形给出能看懂的 400 提示。
"""
from decimal import Decimal as D
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

# 店铺 A / B / C 各 2 个渠道；店铺 D 的金额全是 0（毛利率的除数为 0）
rows = []
for i in range(600):
    shop = "ABC"[i % 3]
    ch = ["直播", "商品卡"][(i // 3) % 2]
    amount = D(i % 97) * D("3.25") + D("10")
    cost = amount * D("0.6") + D(i % 5)
    rows.append((shop, ch, amount, cost, 1 + i % 4))
for _ in range(4):
    rows.append(("D", "直播", D("0"), D("5"), 1))
csv = "店铺,渠道,金额,成本,数量\n" + "\n".join(f"{s},{c},{a},{co:.2f},{q}" for s, c, a, co, q in rows) + "\n"
DS = import_ds(A, "计算值测试", csv)
pivot = lambda body: call("POST", f"/ledger/datasets/{DS}/pivot", A, body=body)

def agg(group):
    sel = [r for r in rows if group(r)]
    amount = sum(r[2] for r in sel); cost = sum(r[3] for r in sel)
    return {"amount": amount, "cost": cost, "n": len(sel), "profit": amount - cost, "rate": (amount - cost) / amount if amount else None}

close = lambda a, b: (a is None and b is None) or (a is not None and b is not None and abs(float(a) - float(b)) < 0.006)
GROSS = {"column": None, "agg": "calc", "label": "毛利", "formula": "[金额（求和）] - [成本（求和）]", "format": "money"}
RATE = {"column": None, "agg": "calc", "label": "毛利率", "formula": "([金额]-[成本]) / [金额]", "format": "percent"}
VALUES = [{"column": "金额", "agg": "sum"}, {"column": "成本", "agg": "sum"}, GROSS, RATE]

# ── 1. 一级行：每行、总计都与独立计算一致 ──
s, r = pivot({"rows": [{"column": "店铺"}], "values": VALUES})
check("计算值请求成功", s == 200, (s, r))
res = r["data"]
check("值的说明：标签、类型", [(v["label"], v["kind"]) for v in res["values"]] == [("金额（求和）", "money"), ("成本（求和）", "money"), ("毛利", "money"), ("毛利率", "percent")], res["values"])
check("计算值的 agg 是 calc、没有列", res["values"][2]["agg"] == "calc" and res["values"][2]["column"] is None, res["values"][2])
data = {x["keys"][0]: x["cells"][-1] for x in res["rows"] if x["kind"] == "data"}
ok = True
for shop in "ABCD":
    e = agg(lambda r_, s_=shop: r_[0] == s_)
    c = data[shop]
    ok &= close(c[2], e["profit"]) and close(c[3], e["rate"])
check("每个店铺：毛利 = 金额 - 成本，毛利率 = 毛利 / 金额", ok, data)
check("除数为 0（店铺 D 金额全是 0）：毛利率为空，毛利仍有值（-20）", data["D"][3] is None and close(data["D"][2], -20), data["D"])
tot = [x for x in res["rows"] if x["kind"] == "total"][0]["cells"][-1]
e = agg(lambda r_: True)
check("总计行的毛利率是「总毛利 / 总金额」", close(tot[2], e["profit"]) and close(tot[3], e["rate"]), (tot, e["rate"]))

# ── 2. 两级行：小计行按小计汇总值计算 ──
s, r = pivot({"rows": [{"column": "店铺"}, {"column": "渠道"}], "values": VALUES})
res = r["data"]
subs = {x["keys"][0]: x["cells"][-1] for x in res["rows"] if x["kind"] == "subtotal"}
ok = all(close(subs[sh][2], agg(lambda r_, s_=sh: r_[0] == s_)["profit"]) and close(subs[sh][3], agg(lambda r_, s_=sh: r_[0] == s_)["rate"]) for sh in "ABC")
check("小计行：按小计的金额与成本算毛利、毛利率", ok, subs)
leaf = {(x["keys"][0], x["keys"][1]): x["cells"][-1] for x in res["rows"] if x["kind"] == "data"}
ok = all(close(leaf[(sh, ch)][3], agg(lambda r_, s_=sh, c_=ch: r_[0] == s_ and r_[1] == c_)["rate"]) for sh in "ABC" for ch in ("直播", "商品卡"))
check("明细行：每个「店铺 × 渠道」的毛利率正确", ok)

# ── 3. 有列字段：每一列、总计列都单独算 ──
s, r = pivot({"rows": [{"column": "店铺"}], "columns": [{"column": "渠道"}], "values": VALUES})
res = r["data"]
keys = [k[0] for k in res["columnKeys"]]
rowA = [x for x in res["rows"] if x["kind"] == "data" and x["keys"][0] == "A"][0]["cells"]
ok = True
for i, ch in enumerate(keys):
    e = agg(lambda r_, c_=ch: r_[0] == "A" and r_[1] == c_)
    ok &= close(rowA[i][2], e["profit"]) and close(rowA[i][3], e["rate"])
e = agg(lambda r_: r_[0] == "A")
ok &= close(rowA[-1][3], e["rate"])
check("列字段：每个渠道列与总计列的毛利率都正确", ok, (keys, rowA))

# ── 4. 按自定义表头引用、客单价 ──
s, r = pivot({"rows": [{"column": "店铺"}], "values": [{"column": "金额", "agg": "sum"}, {"column": None, "agg": "count", "label": "订单数"}, {"column": None, "agg": "calc", "label": "客单价", "formula": "[金额（求和）] / [订单数]", "format": "money"}]})
res = r["data"]
data = {x["keys"][0]: x["cells"][-1] for x in res["rows"] if x["kind"] == "data"}
e = agg(lambda r_: r_[0] == "A")
check("按自定义表头「订单数」引用；客单价 = 金额 / 订单数", close(data["A"][2], e["amount"] / e["n"]), data["A"])
check("格式 number 为默认（不写 format）", res["values"][2]["kind"] == "money")

# ── 5. 按计算值排序 ──
s, r = pivot({"rows": [{"column": "店铺"}], "values": VALUES, "sort": {"by": "value", "valueIndex": 2, "desc": True}})
order = [x["keys"][0] for x in r["data"]["rows"] if x["kind"] == "data"]
want = sorted("ABCD", key=lambda sh: -float(agg(lambda r_, s_=sh: r_[0] == s_)["profit"]))
check("按毛利（计算值）降序排序", s == 200 and order == want, (order, want))

# ── 6. 写错时的提示 ──
def bad(name, body, contains):
    s, r = pivot(body)
    check(name, s == 400 and contains in (r.get("msg") or ""), (s, r.get("msg")))
base = {"rows": [{"column": "店铺"}]}
bad("引用不存在的值：说明可用的值", {**base, "values": [{"column": "金额", "agg": "sum"}, {"agg": "calc", "formula": "[利润] * 2"}]}, "不是透视表里的值（可用：[金额（求和）]）")
bad("缺少公式", {**base, "values": [{"column": "金额", "agg": "sum"}, {"agg": "calc", "label": "空公式"}]}, "缺少公式")
bad("公式语法错误：括号", {**base, "values": [{"column": "金额", "agg": "sum"}, {"agg": "calc", "formula": "([金额]"}]}, "括号")
bad("计算值不能引用另一个计算值", {**base, "values": [{"column": "金额", "agg": "sum"}, {"agg": "calc", "label": "甲", "formula": "[金额]*2"}, {"agg": "calc", "formula": "[甲]+1"}]}, "是另一个计算值")
bad("计算值不能设占比", {**base, "values": [{"column": "金额", "agg": "sum"}, {"agg": "calc", "formula": "[金额]*2", "show": "pct_total"}]}, "不能再设置占比")
bad("只有计算值没有普通值", {**base, "values": [{"agg": "calc", "formula": "1+1"}]}, "至少添加一个普通的值字段")
bad("前 N 项按第一个值排名，第一个值不能是计算值", {**base, "top": 2, "values": [{"agg": "calc", "formula": "1+1"}, {"column": "金额", "agg": "sum"}]}, "第一个值不能是计算值")
bad("格式不认识", {**base, "values": [{"column": "金额", "agg": "sum"}, {"agg": "calc", "formula": "[金额]", "format": "excel"}]}, "money / number / percent")

# ── 7. 不影响原有汇总：没有计算值时结果与以前一致 ──
s, r = pivot({"rows": [{"column": "店铺"}], "values": [{"column": "金额", "agg": "sum"}]})
data = {x["keys"][0]: x["cells"][-1][0] for x in r["data"]["rows"] if x["kind"] == "data"}
check("没有计算值时照旧", s == 200 and close(data["A"], agg(lambda r_: r_[0] == "A")["amount"]), data)
finish()
