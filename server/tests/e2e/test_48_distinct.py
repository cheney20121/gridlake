"""公式下推：DCOUNTDISTINCTIFS（服务端 aggregate 接口的 distinct）

订单（客户列有重名、有空值）：
  1 A c1 10 | 2 A c1 20 | 3 A c2 5 | 4 B c1 100 | 5 B c3 200 | 6 C （空）1 | 7 B c3 7
c1 同时出现在 A、B 两家：各店铺的去重数加起来（2 + 2）比总去重数（3）大——证明不能简单把分组结果相加。
"""
from lib import *  # noqa: F403

call("GET", "/ledger/me", A)
s, r = call("POST", "/ledger/datasets/input", A, body={"name": "订单", "columns": [
    {"title": "订单号", "kind": "text"}, {"title": "店铺", "kind": "text"}, {"title": "客户", "kind": "text"}, {"title": "金额", "kind": "money"}], "primaryKey": ["订单号"]})
assert s == 200, (s, r)
DS = r["data"]
rows = [["1", "A", "c1", 10], ["2", "A", "c1", 20], ["3", "A", "c2", 5], ["4", "B", "c1", 100], ["5", "B", "c3", 200], ["6", "C", None, 1], ["7", "B", "c3", 7]]
s, r = call("POST", f"/ledger/datasets/{DS}/writeback", A, body={"mode": "upsert", "columns": ["订单号", "店铺", "客户", "金额"], "rows": rows})
assert s == 200 and r["data"]["inserted"] == 7, (s, r)


def agg(*calls):
    s, r = call("POST", f"/ledger/datasets/{DS}/aggregate", A, body={"calls": list(calls)})
    assert s == 200, (s, r)
    return r["data"]["values"], r["data"]["errors"]


c = lambda func, col, *crit: {"func": func, "column": col, "criteria": [{"column": k, "value": v} for k, v in crit]}
d = lambda col, *crit: c("distinct", col, *crit)

vals, errs = agg(
    d("客户"),                                   # 0 全表：c1 c2 c3（空值不算）
    d("客户", ("店铺", "A")), d("客户", ("店铺", "B")), d("客户", ("店铺", "C")), d("客户", ("店铺", "Z")),   # 1-4
    d("客户", ("店铺", "*")),                    # 5 通配符：三家合并去重，不是相加
    d("客户", ("店铺", "<>C")),                  # 6 不等：A、B 合并
    d("客户", ("店铺", "a")),                    # 7 条件不分大小写
    d("客户", ("店铺", "A"), ("金额", ">=10")),   # 8 数值条件
    d("客户", ("金额", "<150")),                 # 9 只有数值条件
    d("订单号"), d("订单号", ("店铺", "B")),      # 10-11
    d("金额", ("店铺", "A")),                    # 12 数值列也能去重
    d("店铺", ("客户", "c1")),                   # 13 条件在另一列
    d("店铺", ("客户", "")),                     # 14 空值条件
    d("客户", ("店铺", "A"), ("客户", "c1")),     # 15 去重列同时是条件列
    d("店铺", ("店铺", "*")),                    # 16 通配符 + 去重列自身
)
check("全表去重客户数：3（空值不算一个取值）", vals[0] == 3, (vals, errs))
check("按店铺精确条件：A 店 2、B 店 2、C 店 0、没有的店铺 0", vals[1:5] == [2, 2, 0, 0], vals)
check("通配符：各店铺合并后去重是 3，不是 2+2+0", vals[5] == 3, vals)
check("不等于 C：A、B 合并去重是 3", vals[6] == 3, vals)
check("条件不分大小写", vals[7] == 2, vals)
check("文本条件 + 数值条件：A 店金额>=10 只有 c1", vals[8] == 1, vals)
check("只有数值条件：金额<150 的客户有 c1 c2 c3", vals[9] == 3, vals)
check("订单号去重：全部 7 个，B 店 3 个", vals[10] == 7 and vals[11] == 3, vals)
check("数值列去重：A 店的金额 10、20、5 共 3 个", vals[12] == 3, vals)
check("条件在另一列：客户 c1 出现在 A、B 两家店", vals[13] == 2, vals)
check("空值条件：客户为空的订单只在 C 店", vals[14] == 1, vals)
check("去重列同时是条件列", vals[15] == 1, vals)
check("通配符加去重列自身：三家店", vals[16] == 3, vals)
check("没有报错", all(e is None for e in errs), errs)

# 和其他函数混在同一批里也行
vals, errs = agg(d("客户", ("店铺", "B")), c("sum", "金额", ("店铺", "B")), c("count", None, ("店铺", "B")), d("客户", ("店铺", "*B*")), c("max", "金额", ("店铺", "A")))
check("与求和 / 计数 / 最大值同批计算互不影响", vals == [2, 307, 3, 2, 20], (vals, errs))

vals, errs = agg(d("不存在的列"), d("客户", ("没有这列", "x")), {"func": "distinct", "criteria": []})
check("列不存在报错", errs[0] and "没有列" in errs[0] and errs[1] and "没有列" in errs[1], errs)
check("缺少去重列报错", errs[2] is not None, errs)

# 新增数据后重新计算（缓存由前端管理，服务端每次都是现算）
s, r = call("POST", f"/ledger/datasets/{DS}/writeback", A, body={"mode": "upsert", "columns": ["订单号", "店铺", "客户", "金额"], "rows": [["8", "A", "c9", 1], ["9", "A", "C1", 1]]})
vals, errs = agg(d("客户", ("店铺", "A")), d("客户"))
check("去重区分大小写的取值（c1 与 C1 是两个）：A 店 4、全表 5", vals == [4, 5], (vals, errs))
finish()
