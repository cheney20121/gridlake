"""修改已有数据集的列类型：预览（多少值转不过来、举例）→ 有损时必须确认 → 就地转换；
只有所有者与管理员能改；改完筛选、求和、关键字搜索按新类型；引用它的关联视图照常能用；视图 / 外部数据集不能改。
"""
from lib import *  # noqa: F403

for t in (A, Bo, R): call("GET", "/ledger/me", t)
boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()

# 「单号」是 6 位数字，导入时会被当成整数；「金额」里混了一个「待定」，被当成文本
csv = "单号,店铺,金额\n000123,A,10\n000124,A,20.5\n000125,B,待定\n000126,B,100\n"
s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("订单.csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
assert s == 200, (s, r)
p = r["data"]
s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": "create", "name": "订单", "columns": [{"source": c["source"], "title": c["title"], "kind": "integer" if c["title"] == "单号" else c["kind"]} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
assert s == 200, (s, r)
D = r["data"]["datasetId"]
kinds = lambda: {c["title"]: c["kind"] for c in next(d for d in call("GET", "/ledger/datasets", A)[1]["data"] if d["id"] == D)["columns"]}
check("导入后：单号被当成整数、金额是文本", kinds()["单号"] == "integer" and kinds()["金额"] == "text", kinds())

body = lambda col, kind, accept=False: {"column": col, "kind": kind, "acceptLoss": accept}
s, r = call("PUT", f"/ledger/datasets/{D}/columns/kind", Bo, body=body("单号", "id"))
check("不是所有者：不能改", s == 403, (s, r))

# 整数 → 编号：前导零已经在导入时丢了（000123 → 123），改成编号后至少不再参与求和、能按文本搜索
s, r = call("POST", f"/ledger/datasets/{D}/columns/kind/preview", A, body=body("单号", "id"))
check("预览：4 个值都能转换", s == 200 and r["data"]["values"] == 4 and r["data"]["lost"] == 0 and r["data"]["applied"] is False, (s, r))
s, r = call("PUT", f"/ledger/datasets/{D}/columns/kind", A, body=body("单号", "id"))
check("整数 → 编号", s == 200 and r["data"]["applied"] and kinds()["单号"] == "id", (s, r))
spec = lambda **k: {"filters": [], "keyword": None, "sort": [], "rule": None, **k}
s, r = call("POST", f"/ledger/datasets/{D}/cursor", A, body=spec(keyword="125"))
check("编号列参与关键字搜索", s == 200 and r["data"]["total"] == 1, (s, r))

# 文本 → 金额：「待定」转不过来，必须确认
s, r = call("POST", f"/ledger/datasets/{D}/columns/kind/preview", A, body=body("金额", "money"))
check("预览：1 个值转不过来，举例「待定」", s == 200 and r["data"]["lost"] == 1 and r["data"]["lostSamples"] == ["待定"], (s, r))
s, r = call("PUT", f"/ledger/datasets/{D}/columns/kind", A, body=body("金额", "money"))
check("有损且没确认：拒绝，类型不变", s == 400 and "待定" in msg((s, r)) and kinds()["金额"] == "text", (s, r))

# 先建一个引用它的关联视图，改完类型后视图照常能用
s, r = call("POST", "/ledger/datasets/input", A, body={"name": "店铺", "columns": [{"title": "店铺", "kind": "text"}, {"title": "区域", "kind": "text"}], "primaryKey": ["店铺"]})
S = r["data"]
call("POST", f"/ledger/datasets/{S}/writeback", A, body={"mode": "upsert", "columns": ["店铺", "区域"], "rows": [["A", "华东"], ["B", "华北"]]})
s, r = call("POST", "/ledger/datasets/views", A, body={"name": "订单带区域", "left": D, "right": S, "on": [{"left": "店铺", "right": "店铺"}], "columns": [{"source": "区域", "title": "区域"}]})
check("建关联视图", s == 200, (s, r))
V = r["data"]

s, r = call("PUT", f"/ledger/datasets/{D}/columns/kind", A, body=body("金额", "money", True))
check("确认后执行", s == 200 and r["data"]["applied"] and kinds()["金额"] == "money", (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/aggregate", A, body={"calls": [{"func": "sum", "column": "金额", "criteria": [{"column": "店铺", "value": "A"}]}]})
check("改成金额后可以求和", s == 200 and r["data"]["values"] == [30.5], (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/cursor", A, body=spec(filters=[{"op": "between", "column": "金额", "min": 15, "max": 1000}]))
check("按数值区间筛选", s == 200 and r["data"]["total"] == 2, (s, r))
s, r = call("POST", f"/ledger/datasets/{V}/aggregate", A, body={"calls": [{"func": "sum", "column": "金额", "criteria": [{"column": "区域", "value": "华北"}]}]})
check("关联视图照常能用（新类型也带过去了）", s == 200 and r["data"]["values"] == [100], (s, r))

s, r = call("PUT", f"/ledger/datasets/{V}/columns/kind", A, body=body("区域", "id"))
check("关联视图不能改列类型", s == 400 and "不是导入" in msg((s, r)), (s, r))
s, r = call("PUT", f"/ledger/datasets/{D}/columns/kind", A, body=body("店铺", "datetime"))
check("文本 → 日期时间：认不出的保留原文，不算转不过来", s == 200 and r["data"]["kept"] == 4 and r["data"]["lost"] == 0, (s, r))
s, r = call("PUT", f"/ledger/datasets/{D}/columns/kind", R, body=body("店铺", "text"))
check("管理员也能改（改回文本）", s == 200 and kinds()["店铺"] == "text", (s, r))
finish()
