"""列类型：整数（BIGINT）与是否（BOOLEAN）从导入一路用到筛选、透视、公式下推、导出、SQL 另存

订单：数量是整数（含千分位写法），已付款是 true / false；另有一列超过 2^53 的流水号，手动选成整数。
"""
from lib import *  # noqa: F403

call("GET", "/ledger/me", A)
boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()

rows = [(f"O{i:03d}", "AB"[i % 2], 1 + i % 5, i % 3 == 0, 9007199254740993 + i) for i in range(30)]
csv = "单号,店铺,数量,已付款,流水号\n" + "\n".join(f"{o},{s},{q},{str(p).lower()},{n}" for o, s, q, p, n in rows) + "\nO999,B,\"1,000\",FALSE,9007199254740000\n"
rows.append(("O999", "B", 1000, False, 9007199254740000))

s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("订单.csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
assert s == 200, (s, r)
p = r["data"]
kinds = {c["title"]: c["kind"] for c in p["columns"]}
check("预览推断：数量为整数、已付款为是否、流水号（16 位数字）为编号", kinds == {"单号": "text", "店铺": "text", "数量": "integer", "已付款": "boolean", "流水号": "id"}, kinds)
cols = [{"source": c["source"], "title": c["title"], "kind": "integer" if c["title"] == "流水号" else c["kind"]} for c in p["columns"]]
s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": "create", "name": "订单", "columns": cols, "primaryKey": ["单号"], "fileName": p["fileName"]})
check("导入成功，没有无法识别的值", s == 200 and r["data"]["inserted"] == len(rows) and r["data"]["invalid"] == [], (s, r))
D = r["data"]["datasetId"]

s, r = call("GET", "/ledger/datasets", A)
info = next(d for d in r["data"] if d["id"] == D)
check("数据集列类型原样登记", {c["title"]: c["kind"] for c in info["columns"]} == {"单号": "text", "店铺": "text", "数量": "integer", "已付款": "boolean", "流水号": "integer"}, info["columns"])

# ── 取数：是否列为布尔值；超过 2^53 的整数按文本下发（浏览器 JSON 数字会丢位） ──
spec = lambda **k: {"filters": [], "keyword": None, "sort": [], "rule": None, **k}
s, r = call("POST", f"/ledger/datasets/{D}/cursor", A, body=spec(sort=[{"column": "单号", "desc": False}]))
assert s == 200, (s, r)
c = r["data"]
s, r = call("GET", f"/ledger/datasets/{D}/rows?cursor={c['cursorId']}&offset=0&limit=2", A)
first = r["data"]["rows"][0]
check("第一行：数量是数字、已付款是 true、流水号一位不差", first == ["O000", "A", 1, True, "9007199254740993"], first)

# ── 筛选：整数列按数值区间，是否列按值勾选 ──
def count(filters):
    s, r = call("POST", f"/ledger/datasets/{D}/cursor", A, body=spec(filters=filters))
    assert s == 200, (s, r)
    return r["data"]["total"]
check("数量 between 4 ~ 1000", count([{"op": "between", "column": "数量", "min": 4, "max": 1000}]) == sum(1 for x in rows if 4 <= x[2] <= 1000))
check("已付款 = true", count([{"op": "in", "column": "已付款", "values": ["true"]}]) == sum(1 for x in rows if x[3]))
s, r = call("POST", f"/ledger/datasets/{D}/distinct", A, body={"spec": spec(), "column": "已付款"})
check("是否列的候选项是 true / false", s == 200 and sorted(v["value"] for v in r["data"]["values"]) == ["false", "true"], (s, r))

# ── 选区统计、透视、公式下推 ──
s, r = call("POST", f"/ledger/datasets/{D}/stats", A, body={"cursorId": c["cursorId"], "from": 0, "to": len(rows) - 1, "columns": ["数量"]})
check("状态栏求和按整数列算", s == 200 and r["data"]["sum"] == sum(x[2] for x in rows), (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/pivot", A, body={"rows": [{"column": "已付款"}], "values": [{"column": "数量", "agg": "sum"}, {"column": "数量", "agg": "avg"}]})
check("透视请求成功", s == 200, (s, r))
data = {x["keys"][0]: x["cells"][-1] for x in r["data"]["rows"] if x["kind"] == "data"}
want = {k: [x[2] for x in rows if x[3] == (k == "true")] for k in ("true", "false")}
check("按是否分组求和", {k: v[0] for k, v in data.items()} == {k: sum(v) for k, v in want.items()}, data)
check("值类型：整数求和仍是整数，平均是数值", [v["kind"] for v in r["data"]["values"]] == ["integer", "number"], r["data"]["values"])
s, r = call("POST", f"/ledger/datasets/{D}/aggregate", A, body={"calls": [
    {"func": "sum", "column": "数量", "criteria": [{"column": "已付款", "value": "TRUE"}]},
    {"func": "sum", "column": "数量", "criteria": [{"column": "数量", "value": ">3"}]},
]})
vals = r["data"]["values"]
check("DSUMIFS：整数列求和，是否列可作条件（不区分大小写）", s == 200 and vals == [sum(want["true"]), sum(x[2] for x in rows if x[2] > 3)], (s, r))

# ── 导出 CSV：是否列写 true / false，整数不带小数 ──
s, body = call("GET", f"/ledger/datasets/{D}/export?cursor={c['cursorId']}&format=csv", A, binary=True)
text = body.decode("utf-8-sig") if isinstance(body, bytes) else str(body)
check("导出 CSV", s == 200 and "O000,A,1,true,9007199254740993" in text, text[:200])

# ── SQL 另存：BIGINT / BOOLEAN 结果列成为整数 / 是否列 ──
sql = "SELECT 店铺, sum(数量) AS 件数, bool_or(已付款) AS 有付款, avg(数量) AS 平均件数 FROM 订单 GROUP BY 店铺"
s, r = call("POST", "/ledger/sql/queries", A, body={"name": "按店铺", "sql": sql, "saveAsDataset": "按店铺", "schedule": {"kind": "manual"}})
check("SQL 另存为数据集", s == 200 and r["data"]["datasetId"], (s, r))
s, r = call("GET", "/ledger/datasets", A)
saved = next(d for d in r["data"] if d["name"] == "按店铺")
check("结果列类型：求和为整数、bool_or 为是否、平均为数值", {c["title"]: c["kind"] for c in saved["columns"]} == {"店铺": "text", "件数": "integer", "有付款": "boolean", "平均件数": "number"}, saved["columns"])
s, r = call("POST", f"/ledger/datasets/{saved['id']}/aggregate", A, body={"calls": [{"func": "sum", "column": "件数", "criteria": []}]})
check("另存的整数列可以继续求和", s == 200 and r["data"]["values"] == [sum(x[2] for x in rows)], (s, r))

# ── 填报：整数 / 是否列校验写法 ──
s, r = call("POST", "/ledger/datasets/input", A, body={"name": "盘点", "columns": [{"title": "货号", "kind": "text"}, {"title": "件数", "kind": "integer"}, {"title": "在售", "kind": "boolean"}], "primaryKey": ["货号"]})
check("建填报数据集（含整数、是否列）", s == 200, (s, r))
W = r["data"]
s, r = call("POST", f"/ledger/datasets/{W}/writeback", A, body={"mode": "upsert", "columns": ["货号", "件数", "在售"], "rows": [["X1", "1.5", "是"], ["X2", "3", "也许"]]})
errs = sorted((e["column"], e["row"]) for e in r["data"]["errors"]) if s == 200 else r
check("整数列不收小数、是否列不收别的写法，整批不写入", s == 200 and errs == [("件数", 0), ("在售", 1)] and r["data"]["inserted"] == 0, (s, r))
s, r = call("POST", f"/ledger/datasets/{W}/writeback", A, body={"mode": "upsert", "columns": ["货号", "件数", "在售"], "rows": [["X1", "1,200", "是"], ["X2", 3, False]]})
check("合法写法写入", s == 200 and r["data"]["inserted"] == 2, (s, r))
s, r = call("POST", f"/ledger/datasets/{W}/aggregate", A, body={"calls": [{"func": "sum", "column": "件数", "criteria": [{"column": "在售", "value": "true"}]}]})
check("回写后的值类型正确", s == 200 and r["data"]["values"] == [1200], (s, r))
finish()
