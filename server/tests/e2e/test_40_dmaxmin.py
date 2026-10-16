"""公式下推：DMAXIFS / DMINIFS（服务端 aggregate 接口的 max / min）

订单：A 店 金额 10 / 20 / -5，B 店 100 / 200，C 店 没有数值（金额留空）。
"""
from lib import *  # noqa: F403

call("GET", "/ledger/me", A)
s, r = call("POST", "/ledger/datasets/input", A, body={"name": "订单", "columns": [
    {"title": "订单号", "kind": "text"}, {"title": "店铺", "kind": "text"}, {"title": "金额", "kind": "money"}], "primaryKey": ["订单号"]})
assert s == 200, (s, r)
DS = r["data"]
rows = [["1", "A", 10], ["2", "A", 20], ["3", "A", -5], ["4", "B", 100], ["5", "B", 200], ["6", "C", None]]
s, r = call("POST", f"/ledger/datasets/{DS}/writeback", A, body={"mode": "upsert", "columns": ["订单号", "店铺", "金额"], "rows": rows})
assert s == 200 and r["data"]["inserted"] == 6, (s, r)

def agg(*calls):
    s, r = call("POST", f"/ledger/datasets/{DS}/aggregate", A, body={"calls": list(calls)})
    assert s == 200, (s, r)
    return r["data"]["values"], r["data"]["errors"]

c = lambda func, col, *crit: {"func": func, "column": col, "criteria": [{"column": k, "value": v} for k, v in crit]}
vals, errs = agg(
    c("max", "金额"), c("min", "金额"),
    c("max", "金额", ("店铺", "A")), c("min", "金额", ("店铺", "A")),
    c("max", "金额", ("店铺", "B")), c("min", "金额", ("店铺", "B")),
    c("max", "金额", ("店铺", "C")), c("min", "金额", ("店铺", "Z")),
    c("max", "金额", ("店铺", "<>A")), c("min", "金额", ("金额", ">0")),
    c("max", "金额", ("店铺", "*"), ("金额", "<150")),
)
check("全表最大 200、最小 -5", vals[0] == 200 and vals[1] == -5, (vals, errs))
check("按店铺：A 店 20 / -5，B 店 200 / 100", vals[2:6] == [20, -5, 200, 100], vals)
check("没有数值的店铺、没有这个店铺：和 Excel 一致，为 0", vals[6] == 0 and vals[7] == 0, vals)
check("条件支持 <> 与数值比较：非 A 店最大 200，金额>0 最小 10", vals[8] == 200 and vals[9] == 10, vals)
check("多个条件一起生效：金额 < 150 的最大值是 100", vals[10] == 100, vals)
check("没有报错", all(e is None for e in errs), errs)

vals, errs = agg(c("max", "店铺"), c("min", "不存在的列"))
check("文本列不能取最大值", errs[0] and "不是数值列" in errs[0] and "最大值" in errs[0], errs)
check("列不存在报错", errs[1] and "没有列" in errs[1], errs)
finish()
