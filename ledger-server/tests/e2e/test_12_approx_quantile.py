"""近似分位数（approx_median / approx_p90 / approx_p95 / approx_p99）：与精确分位数对照误差，分组与总计、校验

近似版用 t-digest：常数内存，适合上亿行；这里在 30 万行偏斜分布（大量小额 + 少量极大值）上量化误差，
并确认分组汇总、总计行、显示名称与参数校验正常。
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

random.seed(7)
N = 300_000
rows = []
for i in range(N):
    shop = f"店{i % 5}"
    # 对数正态：多数是几十到几百，少数上万
    amount = round(random.lognormvariate(5.0, 1.2), 2)
    rows.append((shop, amount))
csv = "店铺,金额\n" + "\n".join(f"{s},{a}" for s, a in rows) + "\n"
DS = import_ds(A, "近似分位数", csv)
pivot = lambda body: call("POST", f"/ledger/datasets/{DS}/pivot", A, body=body)

def total(agg):
    s, r = pivot({"rows": [], "values": [{"column": "金额", "agg": agg}]})
    assert s == 200, (s, r)
    return r["data"]["rows"][-1]["cells"][-1][0], r["data"]

vals = sorted(a for _, a in rows)
def rank_of(x):   # 近似值在真实数据里的分位（用二分）
    import bisect
    return bisect.bisect_left(vals, x) / N

for exact, approx, q in [("median", "approx_median", 0.5), ("p90", "approx_p90", 0.9), ("p95", "approx_p95", 0.95), ("p99", "approx_p99", 0.99)]:
    e, _ = total(exact)
    a, data = total(approx)
    rel = abs(a - e) / e
    rank_err = abs(rank_of(a) - q)
    print(f"  {approx}: 精确 {e:.2f} 近似 {a:.2f} 相对误差 {rel:.3%} 名次误差 {rank_err:.3%}")
    check(f"{approx}：名次误差在 0.5% 以内（落在真实数据的第 {q:.0%} ± 0.5% 分位）", rank_err < 0.005, (a, e, rank_err))
    check(f"{approx}：值的相对误差在 3% 以内", rel < 0.03, (a, e, rel))

# 显示名称与类型
_, data = total("approx_p95")
v = data["values"][0]
check("表头显示「金额（近似 P95）」，类型沿用金额", v["label"] == "金额（近似 P95）" and v["kind"] == "money", v)
_, data = total("approx_median")
check("近似中位数的名称", data["values"][0]["label"] == "金额（近似中位数）", data["values"][0])

# 分组与总计行都有值，且各组的近似值接近各组的精确值
s, r1 = pivot({"rows": [{"column": "店铺"}], "values": [{"column": "金额", "agg": "p95"}, {"column": "金额", "agg": "approx_p95"}]})
rows1 = {x["keys"][0]: x["cells"][-1] for x in r1["data"]["rows"] if x["kind"] == "data"}
worst = max(abs(c[1] - c[0]) / c[0] for c in rows1.values())
check("分组：5 家店铺的近似 P95 与精确值相差都在 5% 以内", s == 200 and len(rows1) == 5 and worst < 0.05, (worst, rows1))
tot = [x for x in r1["data"]["rows"] if x["kind"] == "total"][0]["cells"][-1]
check("总计行也有近似值", tot[1] is not None and abs(tot[1] - tot[0]) / tot[0] < 0.03, tot)

# 文本列不能求分位数；和计算值一起用
s, r = pivot({"rows": [], "values": [{"column": "店铺", "agg": "approx_p95"}]})
check("文本列不能求近似分位数", s == 400 and "不是数值列" in (r.get("msg") or ""), (s, r.get("msg")))
s, r = pivot({"rows": [{"column": "店铺"}], "values": [{"column": "金额", "agg": "approx_median"}, {"column": "金额", "agg": "sum"}, {"agg": "calc", "label": "中位数占均值", "formula": "[金额（近似中位数）] / ([金额（求和）] / 60000)", "format": "percent"}]})
check("近似分位数可以被计算值引用", s == 200 and all(x["cells"][-1][2] is not None for x in r["data"]["rows"] if x["kind"] == "data"), (s, r.get("msg")))
finish()
