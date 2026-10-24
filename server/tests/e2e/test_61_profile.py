"""数据概况：每列的空值、不同值个数（取值少的精确）、范围、数值统计、认不出的日期；单列的直方图（数值分箱 / 整数每值一档 /
日期按日月）和最常见的取值；按筛选统计；数据范围受限的用户只统计自己范围内的行。
"""
from lib import *  # noqa: F403

for t in (A, Bo, R):
    call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()

csv = "单号,店铺,金额,件数,下单时间,备注\n" + "".join(
    f"N{i:03d},{'ABC'[i % 3]},{i * 10 if i % 7 else ''},{1 + i % 4},2026-09-{1 + i % 28:02d} 10:00:00,{'  ' if i == 5 else ('加急' if i % 10 == 0 else '')}\n" for i in range(60)
)
s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("订单.csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
assert s == 200, (s, r)
p = r["data"]
kinds = {"单号": "id", "金额": "money", "件数": "integer", "下单时间": "datetime"}
s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": "create", "name": "订单", "columns": [{"source": c["source"], "title": c["title"], "kind": kinds.get(c["title"], c["kind"])} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
assert s == 200, (s, r)
D = r["data"]["datasetId"]

# ── 概况 ──
s, r = call("POST", f"/ledger/datasets/{D}/profile", A, body={})
check("概况：60 行、6 列", s == 200 and r["data"]["rows"] == 60 and len(r["data"]["columns"]) == 6, (s, r))
cols = {c["title"]: c for c in r["data"]["columns"]}
check("单号：每行都不同（精确数）", cols["单号"]["distinct"] == 60 and cols["单号"]["distinctExact"] and cols["单号"]["nulls"] == 0, cols["单号"])
check("店铺：3 种", cols["店铺"]["distinct"] == 3 and cols["店铺"]["minLen"] == 1 and cols["店铺"]["maxLen"] == 1, cols["店铺"])
amounts = [i * 10 for i in range(60) if i % 7]
m = cols["金额"]
check("金额：空值、范围、合计、平均", m["nulls"] == 60 - len(amounts) and m["min"] == 10 and m["max"] == 590 and abs(m["sum"] - sum(amounts)) < 1e-6 and abs(m["mean"] - sum(amounts) / len(amounts)) < 1e-6, m)
check("金额：近似中位数在合理范围、标准差、零和负数", 250 <= m["median"] <= 350 and m["std"] > 0 and m["zeros"] == 0 and m["negatives"] == 0, m)
check("件数：4 种，1 ~ 4", cols["件数"]["distinct"] == 4 and cols["件数"]["min"] == 1 and cols["件数"]["max"] == 4, cols["件数"])
check("下单时间：范围，没有认不出的", cols["下单时间"]["min"] == "2026-09-01 10:00:00" and cols["下单时间"]["max"] == "2026-09-28 10:00:00" and cols["下单时间"]["invalid"] == 0, cols["下单时间"])
check("备注：导入时只有空白的格子按空值入库（54 个空值），1 种取值", cols["备注"]["nulls"] == 54 and cols["备注"]["blanks"] == 0 and cols["备注"]["distinct"] == 1, cols["备注"])
s, r = call("POST", f"/ledger/datasets/{D}/profile", A, body={"columns": ["金额"]})
check("只统计指定的列", s == 200 and [c["title"] for c in r["data"]["columns"]] == ["金额"], (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/profile", A, body={"columns": ["不存在"]})
check("列不存在：报错", s == 400 and "不存在" in msg((s, r)), (s, r))

# ── 单列 ──
s, r = call("POST", f"/ledger/datasets/{D}/profile/column", A, body={"column": "金额"})
d = r["data"]
check("金额：等宽 20 档，加起来等于非空个数", s == 200 and d["granularity"] == "bin" and len(d["histogram"]) == 20 and sum(b["count"] for b in d["histogram"]) == len(amounts), d)
check("金额：每档有区间（可按它筛选），第一档从最小值开始", d["histogram"][0]["from"] == 10 and d["histogram"][-1]["to"] == 590, d["histogram"][:2])
s, r = call("POST", f"/ledger/datasets/{D}/profile/column", A, body={"column": "件数"})
d = r["data"]
check("件数：整数取值少，每个值一档", d["granularity"] == "value" and [b["label"] for b in d["histogram"]] == ["1", "2", "3", "4"] and [b["count"] for b in d["histogram"]] == [15, 15, 15, 15], d["histogram"])
s, r = call("POST", f"/ledger/datasets/{D}/profile/column", A, body={"column": "下单时间"})
d = r["data"]
check("下单时间：跨度一个月以内按日", d["granularity"] == "day" and d["histogram"][0] == {"label": "2026-09-01", "prefix": "2026-09-01", "count": 3}, d["histogram"][:2])
s, r = call("POST", f"/ledger/datasets/{D}/profile/column", A, body={"column": "店铺"})
d = r["data"]
check("店铺：没有直方图，最常见的取值 + 其余", d["histogram"] == [] and {t["value"]: t["count"] for t in d["top"]} == {"A": 20, "B": 20, "C": 20} and d["others"] == 0, d)
s, r = call("POST", f"/ledger/datasets/{D}/profile/column", A, body={"column": "单号"})
check("单号：最多列出 20 个，其余计数", len(r["data"]["top"]) == 20 and r["data"]["others"] == 40, r["data"]["others"])

# ── 按筛选 ──
flt = {"filters": [{"op": "in", "column": "店铺", "values": ["A"]}]}
s, r = call("POST", f"/ledger/datasets/{D}/profile", A, body=flt)
cols = {c["title"]: c for c in r["data"]["columns"]}
check("按筛选统计：只看 A 店 20 行", r["data"]["rows"] == 20 and cols["店铺"]["distinct"] == 1, r["data"]["rows"])
s, r = call("POST", f"/ledger/datasets/{D}/profile/column", A, body={**flt, "column": "金额"})
check("单列也按筛选", sum(b["count"] for b in r["data"]["histogram"]) == len([i for i in range(60) if i % 7 and i % 3 == 0]), r["data"]["histogram"])
s, r = call("POST", f"/ledger/datasets/{D}/profile", A, body={"filters": [{"op": "between", "column": "金额", "min": 100, "max": 200}]})
check("区间筛选（直方图一档）", r["data"]["rows"] == len([a for a in amounts if 100 <= a <= 200]), r["data"]["rows"])

# ── 数据范围：Bob 只能看 A 店 ──
call("PUT", f"/ledger/data-scope/datasets/{D}", R, body={"scopeColumn": "店铺", "open": False})
call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["A"]})
s, r = call("POST", f"/ledger/datasets/{D}/profile", Bo, body={})
cols = {c["title"]: c for c in r["data"]["columns"]} if s == 200 else {}
check("受限用户：概况只统计自己范围内的 20 行", s == 200 and r["data"]["rows"] == 20 and cols["店铺"]["distinct"] == 1 and cols["店铺"]["min"] == "A", (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/profile/column", Bo, body={"column": "店铺"})
check("受限用户：单列取值也只有 A", s == 200 and [t["value"] for t in r["data"]["top"]] == ["A"], (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/profile", A, body={})
check("不受限的用户照常是 60 行（缓存不串）", r["data"]["rows"] == 60, r["data"]["rows"])
finish()
