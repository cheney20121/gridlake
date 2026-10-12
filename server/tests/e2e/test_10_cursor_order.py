"""位置表游标：行的位置就是位置表自己的 rowid（不再算 row_number 窗口）

数据量跨多个行组、并行扫描，排序 / 筛选后的顺序必须和 Python 里独立算出的完全一致：
排序（含并列按入库顺序、空值排最后）、只筛选不排序（保持入库顺序）、筛选 + 排序、选区统计、CSV 导出顺序。
"""
from lib import *  # noqa: F403
for t in (A,): call("GET", "/ledger/me", t)
time.sleep(1)

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
def import_ds(tok, name, csv, pk):
    s, r = call("POST", "/ledger/datasets/import/preview", tok, raw=mp(name + ".csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
    assert s == 200, (s, r)
    p = r["data"]
    s, r = call("POST", "/ledger/datasets/import/commit", tok, body={"token": p["token"], "mode": "create", "name": name, "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": pk, "fileName": p["fileName"]})
    assert s == 200, (s, r)
    return r["data"]["datasetId"]

# 30 万行 > 两个行组（每组 12 万多行）；金额只有 500 种取值（大量并列），每 53 行留空
N = 300_000
amount = lambda i: None if i % 53 == 0 else ((i * 7919) % 500) + 0.5
csv = "单号,店铺,金额\n" + "\n".join(f"O{i:06d},店{i % 11},{'' if amount(i) is None else amount(i)}" for i in range(N)) + "\n"
D = import_ds(A, "位置表顺序", csv, ["单号"])
spec = lambda **k: {"filters": [], "keyword": None, "sort": [], "rule": None, **k}
def cur(sp):
    s, r = call("POST", f"/ledger/datasets/{D}/cursor", A, body=sp); assert s == 200, (s, r); return r["data"]
def all_rows(c, total):
    out = []
    for off in range(0, total, 2000):
        s, r = call("GET", f"/ledger/datasets/{D}/rows?cursor={c['cursorId']}&offset={off}&limit=2000", A); assert s == 200, (s, r)
        out += [row[0] for row in r["data"]["rows"]]
    return out

ids = [f"O{i:06d}" for i in range(N)]
# 金额降序（空值最后），并列按入库顺序
ref_desc = sorted(range(N), key=lambda i: (amount(i) is None, -(amount(i) or 0), i))
c = cur(spec(sort=[{"column": "金额", "desc": True}]))
got = all_rows(c, c["total"])
check("降序排序：总行数", c["total"] == N and c["direct"] is False, c)
check("降序排序：30 万行的顺序与独立计算完全一致（并列按入库顺序、空值最后）", got == [ids[i] for i in ref_desc], next((k for k, (a, b) in enumerate(zip(got, [ids[i] for i in ref_desc])) if a != b), None))

ref_asc = sorted(range(N), key=lambda i: (amount(i) is None, amount(i) or 0, i))
c = cur(spec(sort=[{"column": "金额", "desc": False}]))
check("升序排序：顺序一致（空值仍在最后）", all_rows(c, c["total"]) == [ids[i] for i in ref_asc])

# 只筛选不排序：保持入库顺序
sel = [i for i in range(N) if amount(i) is not None and 100 <= amount(i) <= 140]
f = {"op": "between", "column": "金额", "min": 100, "max": 140}
c = cur(spec(filters=[f]))
check("只筛选：行数一致", c["total"] == len(sel) and c["direct"] is False, (c["total"], len(sel)))
check("只筛选：保持入库顺序", all_rows(c, c["total"]) == [ids[i] for i in sel])

# 筛选 + 排序
ref = sorted(sel, key=lambda i: (-amount(i), i))
c2 = cur(spec(filters=[f], sort=[{"column": "金额", "desc": True}]))
check("筛选 + 排序：顺序一致", all_rows(c2, c2["total"]) == [ids[i] for i in ref])

# 选区统计按位置：排序游标里第 1000 ~ 2000 行的金额合计
s, r = call("POST", f"/ledger/datasets/{D}/stats", A, body={"cursorId": c2["cursorId"], "from": 1000, "to": 1999, "columns": ["金额"]})
want = sum(amount(i) for i in ref[1000:2000])
check("选区统计：按排序后的位置取区间（含两端，1000 行）", s == 200 and r["data"]["count"] == 1000 and abs(r["data"]["sum"] - want) < 0.01, (s, r, want))

# CSV 导出的行序与取行一致
s, body = call("GET", f"/ledger/datasets/{D}/export?cursor={c2['cursorId']}&format=csv", A, binary=True)
lines = body.decode("utf-8-sig").strip().split("\n")[1:]
check("CSV 导出：行序与排序后的位置一致", s == 200 and [l.split(",")[0] for l in lines] == [ids[i] for i in ref], (s, len(lines)))
finish()
