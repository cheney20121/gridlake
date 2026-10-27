"""窗口计算：累计、序号、排名、上一行 / 差 / 变化率、移动平均、组内合计 / 占比、每组前 N 行 → 预览 → 生成派生数据集
（列类型按计算定）→ 源数据集追加导入后自动重算；设置不对时说明原因。
"""
from lib import *  # noqa: F403

for t in (A, R):
    call("GET", "/ledger/me", t)

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()

def import_csv(csv, mode, dataset_id=None):
    s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("订单.csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
    assert s == 200, (s, r)
    p = r["data"]
    kinds = {"金额": "money", "数量": "integer", "日期": "datetime"}
    s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": mode, "datasetId": dataset_id, "name": "订单", "columns": [{"source": c["source"], "title": c["title"], "kind": kinds.get(c["title"], c["kind"])} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
    assert s == 200, (s, r)
    return r["data"]["datasetId"]

# A 店四天：10、20、40、30；B 店两天：100、50
D = import_csv("店铺,日期,金额,数量\nA,2026-09-01,10,1\nA,2026-09-02,20,2\nA,2026-09-03,40,4\nA,2026-09-04,30,3\nB,2026-09-01,100,5\nB,2026-09-02,50,5\n", "create")

calcs = [
    {"func": "running_sum", "column": "金额", "name": "累计金额"},
    {"func": "row_number", "name": "第几天"},
    {"func": "rank", "column": "金额", "name": "金额排名"},
    {"func": "prev", "column": "金额", "name": "前一天"},
    {"func": "diff", "column": "金额", "name": "较前一天"},
    {"func": "pct_change", "column": "金额", "name": "变化率"},
    {"func": "moving_avg", "column": "金额", "name": "两天平均", "rows": 2},
    {"func": "group_sum", "column": "金额", "name": "店铺合计"},
    {"func": "share", "column": "金额", "name": "占店铺"},
    {"func": "group_count", "name": "店铺天数"},
]
spec = {"calcs": calcs, "partitionBy": ["店铺"], "orderBy": [{"column": "日期"}]}
s, r = call("POST", f"/ledger/datasets/{D}/window/preview", A, body=spec)
check("预览成功，给出生成的 SQL", s == 200 and r["data"]["sql"].startswith('SELECT *, sum("金额") OVER (PARTITION BY "店铺" ORDER BY "日期"'), (s, r))
res = r["data"]["result"]
names = [c["name"] for c in res["columns"]]
rows = {(x[names.index("店铺")], x[names.index("日期")][:10]): dict(zip(names, x)) for x in res["rows"]}
a3 = rows[("A", "2026-09-03")]
check("累计：A 店到第三天 10+20+40 = 70", a3["累计金额"] == 70, a3)
check("序号与排名（金额大的在前）", a3["第几天"] == 3 and a3["金额排名"] == 1 and rows[("A", "2026-09-01")]["金额排名"] == 4, a3)
check("上一行、差、变化率", a3["前一天"] == 20 and a3["较前一天"] == 20 and abs(a3["变化率"] - 1.0) < 1e-9 and rows[("A", "2026-09-01")]["前一天"] is None, a3)
check("移动平均（两行）", a3["两天平均"] == 30, a3)
check("组内合计、占比、行数", a3["店铺合计"] == 100 and abs(a3["占店铺"] - 0.4) < 1e-9 and a3["店铺天数"] == 4 and rows[("B", "2026-09-02")]["店铺合计"] == 150, a3)

s, r = call("POST", f"/ledger/datasets/{D}/window/preview", A, body={"calcs": [{"func": "rank", "column": "金额", "name": "名次"}], "partitionBy": ["店铺"], "orderBy": [{"column": "金额", "desc": True}], "topN": 1})
top = sorted((x[0], x[2]) for x in r["data"]["result"]["rows"])
check("每组前 1 行：每家店金额最大的一天", s == 200 and top == [("A", 40), ("B", 100)], (s, top))

# ── 校验 ──
bad = lambda body: call("POST", f"/ledger/datasets/{D}/window/preview", A, body=body)
s, r = bad({"calcs": [{"func": "running_sum", "column": "金额", "name": "累计"}]})
check("累计没有排序列：说明要选排序列", s == 400 and "排序列" in msg((s, r)), (s, r))
s, r = bad({"calcs": [{"func": "group_sum", "column": "店铺", "name": "x"}]})
check("值列不是数值：拒绝", s == 400 and "数值列" in msg((s, r)), (s, r))
s, r = bad({"calcs": [{"func": "group_sum", "column": "金额", "name": "金额"}]})
check("新列名和已有列重复：拒绝", s == 400 and "重复" in msg((s, r)), (s, r))

# ── 生成派生数据集 ──
s, r = call("POST", f"/ledger/datasets/{D}/window", A, body={**spec, "name": "订单（窗口）"})
check("生成：保存为 SQL 查询并写入新数据集，源数据集更新后重算", s == 200 and r["data"]["datasetId"] and r["data"]["schedule"] == {"kind": "after_data", "datasetId": D} and r["data"]["lastRows"] == 6, (s, r))
W, Q = r["data"]["datasetId"], r["data"]["id"]
ds = {d["id"]: d for d in call("GET", "/ledger/datasets", A)[1]["data"]}
kinds = {c["title"]: c["kind"] for c in ds[W]["columns"]}
check("新列的类型：累计是金额、排名是整数、占比 / 变化率是数值", kinds["累计金额"] == "money" and kinds["金额排名"] == "integer" and kinds["第几天"] == "integer" and kinds["占店铺"] == "number" and kinds["变化率"] == "number" and kinds["店铺"] == "text", kinds)
agg = lambda ds_id, col, crit=None: call("POST", f"/ledger/datasets/{ds_id}/aggregate", A, body={"calls": [{"func": "max", "column": col, "criteria": crit or []}]})[1]["data"]["values"][0]
check("派生数据集能直接汇总：A 店累计最大 100", agg(W, "累计金额", [{"column": "店铺", "value": "A"}]) == 100, None)

# ── 源数据集追加导入 → 自动重算 ──
import_csv("店铺,日期,金额,数量\nA,2026-09-05,50,5\n", "append", D)
end = time.time() + 20
while time.time() < end and agg(W, "累计金额", [{"column": "店铺", "value": "A"}]) != 150:
    time.sleep(0.3)
check("追加导入后自动重算：A 店累计变成 150", agg(W, "累计金额", [{"column": "店铺", "value": "A"}]) == 150, None)
s, r = call("GET", "/ledger/sql/queries", A)
q = next(x for x in r["data"] if x["id"] == Q)
check("在 SQL 工作台里能看到、能继续改", "窗口计算" in q["description"] and "OVER" in q["sql"], q)
s, r = call("POST", f"/ledger/datasets/{D}/window", A, body={**spec, "name": "订单（窗口二）", "autoRefresh": False})
check("可以不自动重算（手动）", s == 200 and r["data"]["schedule"] == {"kind": "manual"}, (s, r))
finish()
