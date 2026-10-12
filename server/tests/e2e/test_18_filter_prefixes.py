"""筛选条件 prefixes：以其中某个前缀开头（图表点选多个月份时用）

与「prefix」（单个前缀）保持一致的语义："" 表示空值；多个前缀是「或」；与其他筛选条件叠加（且）；空列表不筛选；
日期列是文本时按文本前缀。结果与逐个 prefix 的并集一致，与独立计算的行数一致。
"""
from lib import *  # noqa: F403
for t in (A,): call("GET", "/ledger/me", t)

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()

# 每个月的订单数不同：2026-01 × 3、2026-02 × 5、2026-03 × 7、2026-04 × 11；另有 2 行没有下单时间
rows = []
for month, n in (("2026-01", 3), ("2026-02", 5), ("2026-03", 7), ("2026-04", 11)):
    for i in range(n):
        rows.append(f"{month}-{i + 1:02d} 10:00:00,{'A' if i % 2 == 0 else 'B'},{100 + i}")
rows += [",A,1", ",B,2"]
csv = "下单时间,店铺,金额\n" + "\n".join(rows) + "\n"
s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("订单.csv", csv), ctype=f"multipart/form-data; boundary={boundary}"); p = r["data"]
check("下单时间识别为日期", [c["kind"] for c in p["columns"]][0] == "datetime", [c["kind"] for c in p["columns"]])
s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": "create", "name": "订单", "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]}); D = r["data"]["datasetId"]

def total(*filters):
    s, r = call("POST", f"/ledger/datasets/{D}/cursor", A, body={"filters": list(filters), "keyword": None, "sort": [], "rule": None})
    assert s == 200, (s, r)
    return r["data"]["total"]

pre = lambda *v: {"op": "prefixes", "column": "下单时间", "values": list(v)}
check("无筛选共 28 行", total() == 28, total())
check("单个前缀与 prefix 一致", total(pre("2026-02")) == 5 == total({"op": "prefix", "column": "下单时间", "value": "2026-02"}), total(pre("2026-02")))
check("两个前缀取并集：1 月 + 3 月 = 10 行", total(pre("2026-01", "2026-03")) == 10, total(pre("2026-01", "2026-03")))
check("三个前缀：2、3、4 月 = 23 行", total(pre("2026-02", "2026-03", "2026-04")) == 23, total(pre("2026-02", "2026-03", "2026-04")))
check("前缀可以更长：精确到某一天", total(pre("2026-03-01", "2026-03-02")) == 2, total(pre("2026-03-01", "2026-03-02")))
check("\"\" 表示空值；与月份混用取并集", total(pre("")) == 2 and total(pre("", "2026-01")) == 5, (total(pre("")), total(pre("", "2026-01"))))
check("空列表不筛选", total(pre()) == 28, total(pre()))
check("没有匹配的前缀 = 0 行", total(pre("2027-01")) == 0, total(pre("2027-01")))
check("与其他条件叠加（且）：1、3 月的 A 店 = 2 + 4 行", total(pre("2026-01", "2026-03"), {"op": "in", "column": "店铺", "values": ["A"]}) == 6, total(pre("2026-01", "2026-03"), {"op": "in", "column": "店铺", "values": ["A"]}))
check("前缀里的通配符按字面理解", total(pre("2026-0_")) == 0 and total(pre("%")) == 0, (total(pre("2026-0_")), total(pre("%"))))
s, r = call("POST", f"/ledger/datasets/{D}/cursor", A, body={"filters": [{"op": "prefixes", "column": "不存在的列", "values": ["x"]}], "keyword": None, "sort": [], "rule": None})
check("列不存在被拒绝", s == 400 and "不存在" in msg((s, r)), (s, r))
finish()
