"""导入时拦截：数据集的校验规则设成「整批拒绝」时，追加 / 替换 / 增量更新的新数据里有不合格的就整批不导入，说明是哪几条；
设成「跳过不合格的行」时只导入合格的，告诉你跳过了几条、各违反了什么。「不能重复」和已有数据一起数（替换时只看新数据，
增量更新时会被覆盖的旧行不算）；新数据之间重复的都跳过。默认（照常导入）不拦。保存规则时不带设置就保留原来的。
"""
from lib import *  # noqa: F403

for t in (A,):
    call("GET", "/ledger/me", t)
boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
KINDS = {"编号": "id", "手机号": "id", "金额": "money"}


def imp(csv, mode, dataset_id=None, name="拦截会员", pk=()):
    s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("会员.csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
    p = r["data"]
    return call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": mode, "datasetId": dataset_id, "name": name, "columns": [{"source": c["source"], "title": c["title"], "kind": KINDS.get(c["title"], "text")} for c in p["columns"]], "primaryKey": list(pk), "fileName": p["fileName"]})


def rows(d):
    return next(x for x in call("GET", "/ledger/datasets", A)[1]["data"] if x["id"] == d)["rowCount"]


def values(d, column="编号"):
    return sorted(v["value"] for v in call("POST", f"/ledger/datasets/{d}/distinct", A, body={"filters": [], "column": column})[1]["data"]["values"])


RULES = [{"column": "编号", "kind": "unique"}, {"column": "手机号", "kind": "format", "format": "phone"}, {"column": "金额", "kind": "range", "min": 0}]
s, r = imp("编号,手机号,金额\nA1,13800000001,10\nA2,13800000002,20\n", "create")
D = r["data"]["datasetId"]
s, r = call("PUT", f"/ledger/datasets/{D}/validation", A, body={"rules": RULES, "onImport": "reject"})
check("保存规则和「整批拒绝」", s == 200 and r["data"]["onImport"] == "reject", r)
check("读回设置", call("GET", f"/ledger/datasets/{D}/validation", A)[1]["data"]["onImport"] == "reject", "")

BAD = "编号,手机号,金额\nA3,13800000003,30\nA1,13800000004,40\nA4,123,50\nA5,13800000005,-1\n"
s, r = imp(BAD, "append", D)
m = msg((s, r))
check("整批拒绝：有不合格的就一条都不导入", s == 400 and rows(D) == 2, (s, r))
check("说明几条、违反了什么、是第几条数据", "有 3 条数据不符合「拦截会员」的校验规则" in m and "「编号」不能重复 1 条（第 2 条数据）" in m and "「手机号」是手机号格式 1 条（第 3 条数据）" in m and "「金额」不小于 0 1 条（第 4 条数据）" in m, m)

s, r = call("PUT", f"/ledger/datasets/{D}/validation", A, body={"rules": RULES, "onImport": "skip"})
check("改成「跳过不合格的行」", s == 200 and r["data"]["onImport"] == "skip", r)
s, r = imp(BAD, "append", D)
o = r["data"] if s == 200 else {}
check("跳过：只导入合格的 1 条，跳过 3 条", s == 200 and o["inserted"] == 1 and o["rejected"] == 3 and o["skipped"] == 0 and rows(D) == 3 and values(D) == ["A1", "A2", "A3"], (s, r))
check("每条规则拦下几条", [(h["label"], h["count"], h["lines"]) for h in o.get("rejectedRules", [])] == [("「编号」不能重复", 1, [2]), ("「手机号」是手机号格式", 1, [3]), ("「金额」不小于 0", 1, [4])], o.get("rejectedRules"))

s, r = imp("编号,手机号,金额\nB1,13800000006,1\nB1,13800000007,2\nB2,13800000008,3\n", "append", D)
check("新数据之间重复：都跳过（不替你挑留哪一条）", s == 200 and r["data"]["inserted"] == 1 and r["data"]["rejected"] == 2 and "B1" not in values(D), r)

s, r = call("PUT", f"/ledger/datasets/{D}/validation", A, body={"rules": RULES})
check("保存规则时不带设置：保留原来的", r["data"]["onImport"] == "skip", r["data"].get("onImport"))

s, r = imp("编号,手机号,金额\nA1,13800000009,9\nA9,13800000010,8\n", "replace", D)
check("替换：只和新数据比重复（旧数据都会被替换掉）", s == 200 and r["data"]["inserted"] == 2 and r["data"]["rejected"] == 0 and values(D) == ["A1", "A9"], r)

# ── 增量更新：会被覆盖的旧行不算重复 ──
s, r = imp("编号,手机号,金额\nK1,13800000011,1\nK2,13800000012,2\n", "create", name="增量会员", pk=("编号",))
U = r["data"]["datasetId"]
call("PUT", f"/ledger/datasets/{U}/validation", A, body={"rules": RULES, "onImport": "reject"})
s, r = imp("编号,手机号,金额\nK1,13800000011,100\nK3,13800000013,3\n", "upsert", U, pk=("编号",))
check("增量更新：同主键覆盖旧行不算重复", s == 200 and r["data"]["updated"] == 1 and r["data"]["inserted"] == 1 and rows(U) == 3, (s, r))
s, r = imp("编号,手机号,金额\nK2,1380,5\n", "upsert", U, pk=("编号",))
check("增量更新也会拦：格式不对整批拒绝", s == 400 and "「手机号」是手机号格式 1 条" in msg((s, r)), (s, r))

# ── 默认：照常导入，导入后检查 ──
s, r = imp("编号,手机号,金额\nC1,13800000021,1\n", "create", name="照常会员")
C = r["data"]["datasetId"]
s, r = call("PUT", f"/ledger/datasets/{C}/validation", A, body={"rules": RULES})
check("默认是照常导入", r["data"]["onImport"] == "check", r["data"].get("onImport"))
s, r = imp("编号,手机号,金额\nC2,1,-5\n", "append", C)
check("照常导入：不合格的也导入，导入后检查出来", s == 200 and r["data"]["inserted"] == 1 and r["data"]["rejected"] == 0, r)
time.sleep(1)
v = call("GET", f"/ledger/datasets/{C}/validation", A)[1]["data"]
check("导入后的检查结果里有不合格的", v["report"] and v["report"]["failed"] == 2, v["report"])
s, r = call("PUT", f"/ledger/datasets/{C}/validation", A, body={"rules": RULES, "onImport": "maybe"})
check("设置写错：拒绝", s == 400, (s, r))
finish()
