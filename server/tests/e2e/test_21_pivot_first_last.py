"""透视「首个 / 末个」汇总：按行顺序（导入顺序）取第一个 / 最后一个非空值

典型用法：按店铺看期初余额（首个）与期末余额（末个），再用计算值求「变动 = 末个 - 首个」。
校验：空值跳过、小计 / 总计行按同样口径取值、列字段下每格各取各的、金额保留到分、表头名称、
可被计算值引用、作为「前 N 项」的排名依据、文本列报错。
"""
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

# 行顺序即导入顺序；B 店第一行、A 店第三行的余额为空
ROWS = [
    ("A", "2026-10-01", "100"),
    ("B", "2026-10-01", ""),
    ("A", "2026-10-02", "120.5"),
    ("B", "2026-10-02", "50"),
    ("A", "2026-10-03", ""),
    ("B", "2026-10-03", "70.25"),
    ("C", "2026-10-03", "9"),
]
csv = "店铺,日期,余额\n" + "\n".join(",".join(r) for r in ROWS) + "\n"
DS = import_ds(A, "余额快照", csv)
pivot = lambda body: call("POST", f"/ledger/datasets/{DS}/pivot", A, body=body)

def by_shop(data, ci=-1):
    return {x["keys"][0]: x["cells"][ci] for x in data["rows"] if x["kind"] == "data"}
def total_row(data):
    return [x for x in data["rows"] if x["kind"] == "total"][0]

s, r = pivot({"rows": [{"column": "店铺"}], "values": [{"column": "余额", "agg": "first"}, {"column": "余额", "agg": "last"}]})
check("首个 / 末个可以计算", s == 200, (s, r))
d = r["data"]
shops = by_shop(d)
check("首个：A 100、B 跳过空值取 50、C 9", [shops[k][0] for k in "ABC"] == [100, 50, 9], shops)
check("末个：A 跳过空值取 120.5、B 70.25、C 9", [shops[k][1] for k in "ABC"] == [120.5, 70.25, 9], shops)
tot = total_row(d)["cells"][-1]
check("总计行：全部数据的首个 100、末个 9", tot == [100, 9], tot)
labels = [v["label"] for v in d["values"]]
check("表头「余额（首个）」「余额（末个）」，类型沿用金额 / 数字", labels == ["余额（首个）", "余额（末个）"] and d["values"][0]["kind"] in ("money", "number"), d["values"])

# 列字段：每个格子各取各的
s, r = pivot({"rows": [{"column": "店铺"}], "columns": [{"column": "日期", "granularity": "day"}], "values": [{"column": "余额", "agg": "last"}]})
d = r["data"]
ck = [k[0] for k in d["columnKeys"]]
cell = lambda shop, day: by_shop(d, ck.index(day))[shop][0]
check("列字段：A 店 10-03 只有空值 → 空", s == 200 and cell("A", "2026-10-03") is None, (s, d.get("columnKeys")))
check("列字段：B 店 10-01 空、10-02 为 50", cell("B", "2026-10-01") is None and cell("B", "2026-10-02") == 50, by_shop(d, ck.index("2026-10-02")))
check("列字段：总计列仍是整行的末个（A 120.5）", by_shop(d)["A"][0] == 120.5, by_shop(d))
col_tot = total_row(d)["cells"][ck.index("2026-10-03")][0]
check("列字段：总计行在 10-03 列取该列最后一个非空值 9", col_tot == 9, col_tot)

# 两级行：小计按同样口径
s, r = pivot({"rows": [{"column": "店铺"}, {"column": "日期"}], "values": [{"column": "余额", "agg": "first"}]})
subs = {x["keys"][0]: x["cells"][-1][0] for x in r["data"]["rows"] if x["kind"] == "subtotal"}
check("小计行：B 店的首个 50", s == 200 and subs.get("B") == 50, subs)

# 计算值：变动 = 末个 - 首个
s, r = pivot({"rows": [{"column": "店铺"}], "values": [
    {"column": "余额", "agg": "first"}, {"column": "余额", "agg": "last"},
    {"agg": "calc", "label": "变动", "formula": "[余额（末个）] - [余额（首个）]"},
]})
shops = by_shop(r["data"])
check("计算值引用首个 / 末个：A 店变动 20.5、B 店 20.25、C 店 0", s == 200 and [shops[k][2] for k in "ABC"] == [20.5, 20.25, 0], (s, shops))

# 前 N 项：按首个排名
s, r = pivot({"rows": [{"column": "店铺"}], "values": [{"column": "余额", "agg": "first"}], "top": 2})
check("前 2 项按首个排名：A（100）与 B（50）", s == 200 and sorted(by_shop(r["data"])) == ["A", "B"], (s, r.get("msg") if s != 200 else by_shop(r["data"])))

# 文本列
s, r = pivot({"rows": [], "values": [{"column": "店铺", "agg": "last"}]})
check("文本列不能取首个 / 末个", s == 400 and "不是数值列" in (r.get("msg") or ""), (s, r.get("msg")))

# 追加导入后，新行排在后面，末个随之变化
s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("追加.csv", "店铺,日期,余额\nA,2026-10-04,88\n"), ctype=f"multipart/form-data; boundary={boundary}")
p = r["data"]
s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": "append", "datasetId": DS, "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
check("追加导入成功", s == 200, (s, r))
s, r = pivot({"rows": [{"column": "店铺"}], "values": [{"column": "余额", "agg": "first"}, {"column": "余额", "agg": "last"}]})
shops = by_shop(r["data"])
check("追加后 A 店末个变成 88、首个仍是 100", shops["A"] == [100, 88], shops)
finish()
