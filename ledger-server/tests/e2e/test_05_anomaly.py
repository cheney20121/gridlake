"""N8 异常发现

造 60 天的订单（周末只有平日的一半，属于常态），埋两处异常：2026-09-25（周五）全线故障、店铺 C 从 09-24 起一条都没有；
另造一份没有异常的数据集作对照。检查：该报的报、周末与平稳数据不误报、下钻条件取到的行与结论一致、
受限用户只扫描自己范围内的数据。
"""
import datetime

from lib import *  # noqa: F403
for t in (A, Bo, C, R): call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}

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

LAST = datetime.date(2026, 9, 30)
def build(outage, vanish):
    rows = ["下单时间,店铺,金额"]
    for back in range(59, -1, -1):
        day = LAST - datetime.timedelta(days=back)
        weekend = day.weekday() >= 5
        for shop, n in (("A", 5 if weekend else 10), ("B", 5 if weekend else 10), ("C", 3 if weekend else 5)):
            if vanish and shop == "C" and day >= datetime.date(2026, 9, 24):
                continue
            if outage and day == datetime.date(2026, 9, 25):
                n = 3 if shop != "C" else 2
            for i in range(n):
                rows.append(f"{day} {9 + i % 10:02d}:{i:02d}:00,{shop},{100 + (i % 5) * 2 + (day.day % 3)}")
    return "\n".join(rows) + "\n"

orders = import_ds(A, "订单", build(True, True))
calm = import_ds(A, "平稳订单", build(False, False))

s, r = call("GET", f"/ledger/datasets/{orders}/anomalies", A); check("接口可用", s == 200, (s, r))
res = r["data"]
check("时间列与最新日期", res["timeColumn"] == "下单时间" and res["latestDate"] == "2026-09-30", res)
fs = res["findings"]
titles = [f["title"] for f in fs]
print("   发现：", " | ".join(titles))
out = [f for f in fs if f["kind"] in ("volume", "metric") and "2026-09-25" in f["title"]]
check("故障日 09-25 的行数与金额都报出，且偏低", len(out) == 2 and all(f["change"] < -0.5 for f in out), fs)
check("故障日里最严重的是「高」", any(f["severity"] == "high" for f in out), out)
seg = [f for f in fs if f["kind"] == "segment"]
check("店铺 C 最近 7 天没有数据，报「高」", any("「C」" in f["title"] and "没有数据" in f["title"] and f["severity"] == "high" for f in seg), seg)
check("A、B 店没有被误报", not any("「A」" in f["title"] or "「B」" in f["title"] for f in fs), titles)
check("周末不误报（只有故障日一天有走势异常）", {f["title"][:10] for f in fs if f["kind"] != "segment"} == {"2026-09-25"}, titles)
check("按严重程度与偏离排序，最多 12 条", len(fs) <= 12 and fs[0]["severity"] == "high", titles)

# 下钻条件取到的行与结论一致
day = out[0]
s, r = call("POST", f"/ledger/datasets/{orders}/cursor", A, body={"filters": day["drill"]["filters"], "keyword": None, "sort": [], "rule": None})
check("故障日下钻：6 行（A3 + B3，C 店这时已经没有记录）", s == 200 and r["data"]["total"] == 6, (s, r))
s, r = call("POST", f"/ledger/datasets/{orders}/cursor", A, body={"filters": seg[0]["drill"]["filters"], "keyword": None, "sort": [], "rule": None})
check("店铺 C 下钻：最近 7 天 0 行（正是问题所在）", s == 200 and r["data"]["total"] == 0, (s, r))
s, r = call("POST", f"/ledger/datasets/{orders}/cursor", A, body={"filters": [{"op": "in", "column": "店铺", "values": ["C"]}, {"op": "between", "column": "下单时间", "min": "2026-09-23", "max": "2026-09-23 23:59:59"}], "keyword": None, "sort": [], "rule": None})
check("对照：C 店 09-23（周三）还有 5 行", r["data"]["total"] == 5, r)

# 对照数据集：没有异常
s, r = call("GET", f"/ledger/datasets/{calm}/anomalies", A)
check("平稳数据（含周末低谷）没有任何发现", s == 200 and r["data"]["findings"] == [] and r["data"]["note"] is None, r)

# 数据不足 / 没有时间列
short = import_ds(A, "太短", "下单时间,店铺,金额\n2026-09-29 10:00:00,A,1\n2026-09-30 10:00:00,A,2\n")
s, r = call("GET", f"/ledger/datasets/{short}/anomalies", A); check("数据不足 21 天给出说明", s == 200 and r["data"]["findings"] == [] and "天" in (r["data"]["note"] or ""), r)
notime = import_ds(A, "无时间", "店铺,金额\nA,1\nB,2\n")
s, r = call("GET", f"/ledger/datasets/{notime}/anomalies", A); check("没有日期列给出说明", s == 200 and "日期时间列" in (r["data"]["note"] or ""), r)
s, r = call("GET", "/ledger/datasets/nope/anomalies", A); check("数据集不存在 400", s == 400, (s, r))

# 数据范围：Bob 只能看 A、B；订单按店铺列过滤
s, r = call("PUT", f"/ledger/data-scope/datasets/{orders}", R, body={"scopeColumn": "店铺", "open": False}); check("订单按店铺列过滤", s == 200, (s, r))
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["A", "B"]}); check("给 Bob 设置范围 A、B", s == 200, (s, r))
s, r = call("GET", f"/ledger/datasets/{orders}/anomalies", Bo); bf = r["data"]["findings"] if s == 200 else []
check("Bob 能扫描，仍能看到故障日", s == 200 and any("2026-09-25" in f["title"] for f in bf), (s, r))
check("Bob 的结果里没有店铺 C 的任何信息（C 在他的范围外）", not any("「C」" in f["title"] or "C" in [v for fl in f["drill"]["filters"] if fl["op"] == "in" for v in fl["values"]] for f in bf), [f["title"] for f in bf])
bday = [f for f in bf if f["kind"] == "volume" and "2026-09-25" in f["title"]][0]
check("Bob 看到的故障日数字只含 A、B（6 行）", round(bday["actual"]) == 6, bday)
s, r = call("GET", f"/ledger/datasets/{calm}/anomalies", Bo); check("未绑定范围的数据集 Bob 扫描不到", s == 400, (s, r))

finish()
