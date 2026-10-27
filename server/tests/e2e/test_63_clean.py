"""数据清洗：删除重复行、去首尾空白、查找替换、填充空值（填值 / 向下填充）、删除筛选出的行；
先预览（受影响行数、改前改后样例）再执行；撤销最近一次（之后又有改动就不能撤销）；只有所有者 / 管理员；
数据范围受限的用户不能用；清洗后关键字搜索、依赖它的派生数据集跟着更新。
"""
from lib import *  # noqa: F403

for t in (A, Bo, R):
    call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()

def import_csv(csv, mode, dataset_id=None):
    s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("订单.csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
    assert s == 200, (s, r)
    p = r["data"]
    s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": mode, "datasetId": dataset_id, "name": "订单", "columns": [{"source": c["source"], "title": c["title"], "kind": {"金额": "money"}.get(c["title"], "text")} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
    assert s == 200, (s, r)
    return r["data"]["datasetId"]

# 单号 1、2 重复；店铺写法不统一（北京店 / 北京）；金额有空
D = import_csv("单号,店铺,渠道,金额\n1,北京店,直播,10\n1,北京店,直播,10\n2,北京,短视频,\n3,上海店,直播,30\n3,上海店,直播,30\n4,上海,,40\n", "create")
rows = lambda: next(d for d in call("GET", "/ledger/datasets", A)[1]["data"] if d["id"] == D)["rowCount"]
clean = lambda op, tok=A: call("POST", f"/ledger/datasets/{D}/clean", tok, body=op)
pv = lambda op, tok=A: call("POST", f"/ledger/datasets/{D}/clean/preview", tok, body=op)
spec = lambda **k: {"filters": [], "keyword": None, "sort": [], "rule": None, **k}

# ── 权限 ──
s, r = pv({"op": "dedupe"}, Bo)
check("不是所有者：不能清洗", s == 403, (s, r))

# ── 删除重复行 ──
s, r = pv({"op": "dedupe"})
check("预览：6 行里 2 行重复，样例是要删的行", s == 200 and r["data"]["affected"] == 2 and r["data"]["rows"] == 6 and r["data"]["deletes"] and len(r["data"]["samples"]) == 2, (s, r))
check("还没执行", rows() == 6, rows())
s, r = clean({"op": "dedupe"})
check("执行：剩 4 行", s == 200 and r["data"]["applied"] and rows() == 4, (s, r))
s, r = call("GET", f"/ledger/datasets/{D}/clean", A)
check("记下最近一次，可以撤销", s == 200 and r["data"]["canUndo"] and r["data"]["summary"].startswith("删除重复行") and r["data"]["affected"] == 2, (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/clean/undo", A)
check("撤销：6 行回来", s == 200 and rows() == 6, (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/clean/undo", A)
check("不能撤销两次", s == 400 and "已撤销" in msg((s, r)), (s, r))
s, r = clean({"op": "dedupe", "columns": ["单号"]})
check("按「单号」删除重复：剩 4 行", s == 200 and rows() == 4, (s, r))

# ── 查找替换 ──
s, r = pv({"op": "replace", "column": "店铺", "find": "北京", "replace": "北京店", "whole": True})
check("整格等于：只有「北京」这一格，样例给出改前改后", r["data"]["affected"] == 1 and r["data"]["samples"][0]["before"] == ["北京"] and r["data"]["samples"][0]["after"] == ["北京店"], r["data"])
clean({"op": "replace", "column": "店铺", "find": "北京", "replace": "北京店", "whole": True})
clean({"op": "replace", "column": "店铺", "find": "上海", "replace": "上海店", "whole": True})
s, r = call("POST", f"/ledger/datasets/{D}/distinct", A, body={**spec(), "column": "店铺"})
check("写法统一：只剩两种店铺", sorted(v["value"] for v in r["data"]["values"]) == ["上海店", "北京店"], r["data"]["values"])
s, r = pv({"op": "replace", "column": "店铺", "find": "店", "replace": "", "whole": False})
check("包含：替换每一处", r["data"]["affected"] == 4 and r["data"]["samples"][0]["after"] == ["北京"], r["data"]["samples"][:1])

# ── 填充空值 ──
s, r = pv({"op": "fill", "column": "金额", "value": "很多"})
check("填的值和列类型不符：拒绝", s == 400 and "不是金额" in msg((s, r)), (s, r))
s, r = clean({"op": "fill", "column": "金额", "value": "0"})
check("金额空值填 0", s == 200 and r["data"]["affected"] == 1, (s, r))
s, r = clean({"op": "fill", "column": "渠道", "down": True})
check("渠道向下填充（用上一行的值）", s == 200 and r["data"]["affected"] == 1 and r["data"]["samples"][0]["after"] == ["直播"], (s, r))

# ── 去空白 + 关键字搜索 ──
# 导入时已经去掉了首尾空白；数据连接、SQL 写出的数据可能带着空白。这里用替换造一个
import_csv("单号,店铺,渠道,金额\n9,广州店,直播,5\n", "append", D)
clean({"op": "replace", "column": "店铺", "find": "广州店", "replace": "\u3000 广州店 ", "whole": True})
s, r = clean({"op": "trim"})
check("去首尾空白", s == 200 and r["data"]["affected"] == 1, (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/cursor", A, body=spec(filters=[{"op": "in", "column": "店铺", "values": ["广州店"]}]))
check("去空白后能按「广州店」精确筛选", s == 200 and r["data"]["total"] == 1, (s, r))

# ── 删除筛选出的行 ──
s, r = pv({"op": "delete_rows", "spec": spec()})
check("没有筛选条件不能整表删除", s == 400 and "筛选" in msg((s, r)), (s, r))
s, r = clean({"op": "delete_rows", "spec": spec(filters=[{"op": "in", "column": "店铺", "values": ["广州店"]}])})
check("删除筛选出的行", s == 200 and r["data"]["affected"] == 1 and rows() == 4, (s, r))

# ── 之后又有改动：不能撤销 ──
import_csv("单号,店铺,渠道,金额\n10,北京店,直播,1\n", "append", D)
s, r = call("GET", f"/ledger/datasets/{D}/clean", A)
check("之后又导入了数据：最近一次清洗不能撤销，说明原因", not r["data"]["canUndo"] and "又有改动" in r["data"]["reason"], r["data"])
s, r = call("POST", f"/ledger/datasets/{D}/clean/undo", A)
check("撤销被拒绝", s == 400, (s, r))

# ── 派生数据集跟着更新 ──
s, r = call("POST", f"/ledger/datasets/{D}/window", A, body={"calcs": [{"func": "group_sum", "column": "金额", "name": "店铺合计"}], "partitionBy": ["店铺"], "name": "订单汇总"})
W = r["data"]["datasetId"]
total = lambda: call("POST", f"/ledger/datasets/{W}/aggregate", A, body={"calls": [{"func": "max", "column": "店铺合计", "criteria": [{"column": "店铺", "value": "北京店"}]}]})[1]["data"]["values"][0]
before = total()
clean({"op": "delete_rows", "spec": spec(filters=[{"op": "in", "column": "单号", "values": ["10"]}])})
end = time.time() + 20
while time.time() < end and total() == before:
    time.sleep(0.3)
check("清洗后，依赖它的派生数据集自动重算", total() == before - 1, (before, total()))

# ── 视图 / 数据范围 ──
s, r = call("POST", f"/ledger/datasets/{W}/clean/preview", A, body={"op": "dedupe"})
check("SQL 写出的数据集也是数据表，可以清洗", s == 200, (s, r))
call("PUT", f"/ledger/data-scope/datasets/{D}", R, body={"scopeColumn": "店铺", "open": False})
call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["北京店"]})
s, r = pv({"op": "dedupe"}, Bo)
check("数据范围受限的用户不能清洗", s == 403, (s, r))
s, r = clean({"op": "dedupe"}, R)
check("管理员可以", s in (200, 400), (s, r))
time.sleep(2.5)
s, r = call("GET", "/ledger/audit?size=100", R)
check("执行和撤销进审计", {"数据清洗", "撤销数据清洗"} <= {x["action"] for x in r["data"]["items"]}, None)
finish()
