# env: LEDGER_INBOX_ROOTS=@TMP@/inbox
"""外部数据集：直接查询收件目录里的 Parquet 文件，不导入

fixtures/external 下三个文件：9 月 1000 行、10 月 500 行（先放这两个），11 月 10 行且多一列「备注」（之后放进去再刷新）。
"""
import os
import shutil

from lib import *  # noqa: F403

FIX = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures", "external")
INBOX = os.path.join(os.environ["LEDGER_E2E_TMP"], "inbox")
DIR = os.path.join(INBOX, "订单")
os.makedirs(DIR, exist_ok=True)
for f in ("orders-2026-09.parquet", "orders-2026-10.parquet"):
    shutil.copy(os.path.join(FIX, f), DIR)

# 与样例文件的生成规则一致：(单号, 店铺, 金额, 件数, 下单时间, 已退款)；10 月文件里的店铺 / 金额 / 件数按文件内序号算
from datetime import datetime, timedelta
def gen(base, n, start, refund):
    return [(base + i, "店" + "ABC"[i % 3], (i % 50) * 1.5, 1 + i % 4, start + timedelta(hours=i), refund(i)) for i in range(n)]
ROWS = gen(0, 1000, datetime(2026, 9, 1, 8), lambda i: i % 5 == 0) + gen(1000, 500, datetime(2026, 10, 1, 8), lambda i: False)

for t in (A, R):
    call("GET", "/ledger/me", t)
src = {"path": "订单", "pattern": "orders-*.parquet"}

# ── 权限：读服务器上的文件，只有管理员 ──
s, r = call("POST", "/ledger/datasets/external/preview", A, body=src)
check("普通用户不能预览外部文件", s == 403, (s, r))
s, r = call("POST", "/ledger/datasets/external/preview", R, body={"path": "../"})
check("不能越出收件目录", s == 400 and "只能使用以下目录" in msg((s, r)), (s, r))

# ── 预览 ──
s, r = call("POST", "/ledger/datasets/external/preview", R, body=src)
check("预览成功", s == 200, (s, r))
p = r["data"]
check("两个文件、1500 行", p["fileCount"] == 2 and p["rows"] == 1500, p)
kinds = {c["title"]: c["kind"] for c in p["columns"]}
check("列类型按 Parquet 自带类型", kinds == {"单号": "integer", "店铺": "text", "金额": "money", "件数": "integer", "下单时间": "datetime", "已退款": "boolean"}, kinds)
check("日期时间按约定写成文本", p["sampleRows"][1][4] == "2026-09-01 09:00:00", p["sampleRows"][:2])

# ── 创建与浏览 ──
s, r = call("POST", "/ledger/datasets/external", R, body={"name": "外部订单", **src})
check("创建外部数据集", s == 200, (s, r))
D = r["data"]
s, r = call("GET", "/ledger/datasets", R)
info = next(d for d in r["data"] if d["id"] == D)
check("列表：类型 external、1500 行、文件摘要", info["kind"] == "external" and info["rowCount"] == 1500 and info["external"]["fileCount"] == 2, info)

spec = lambda **k: {"filters": [], "keyword": None, "sort": [], "rule": None, **k}
def cur(sp, tok=R):
    s, r = call("POST", f"/ledger/datasets/{D}/cursor", tok, body=sp); assert s == 200, (s, r); return r["data"]
def rows(c, off, n, tok=R):
    s, r = call("GET", f"/ledger/datasets/{D}/rows?cursor={c['cursorId']}&offset={off}&limit={n}", tok); assert s == 200, (s, r); return r["data"]["rows"]
c = cur(spec())
check("直通游标：不建位置表", c["direct"] and c["total"] == 1500, c)
ids = [x[0] for x in rows(c, 995, 10)]
check("跨文件的一屏按文件顺序连续", ids == list(range(995, 1005)), ids)
check("越过末尾只返回剩下的", [x[0] for x in rows(c, 1495, 10)] == list(range(1495, 1500)))

c2 = cur(spec(filters=[{"op": "in", "column": "店铺", "values": ["店B"]}], sort=[{"column": "金额", "desc": True}]))
got = rows(c2, 0, 1000)
want = [x[0] for x in sorted((x for x in ROWS if x[1] == "店B"), key=lambda x: (-x[2], x[0]))]
check("筛选 + 排序（同值按文件、行号兜底）", c2["total"] == 500 and [x[0] for x in got] == want, (c2["total"], [x[0] for x in got[:5]], want[:5]))
s, r = call("POST", f"/ledger/datasets/{D}/stats", R, body={"cursorId": c["cursorId"], "from": 990, "to": 1009, "columns": ["件数"]})
check("选区统计跨文件", s == 200 and r["data"]["sum"] == sum(x[3] for x in ROWS[990:1010]), (s, r))

# ── 计算：透视、公式下推、计算列、SQL ──
s, r = call("POST", f"/ledger/datasets/{D}/pivot", R, body={"rows": [{"column": "下单时间", "granularity": "month"}], "values": [{"column": "件数", "agg": "sum"}]})
months = {x["keys"][0]: x["cells"][-1][0] for x in r["data"]["rows"] if x["kind"] == "data"} if s == 200 else r
by_month = {}
for x in ROWS:
    by_month[x[4].strftime("%Y-%m")] = by_month.get(x[4].strftime("%Y-%m"), 0) + x[3]
check("透视按月分组（日期时间列照常用）", months == by_month, (months, by_month))
s, r = call("POST", f"/ledger/datasets/{D}/aggregate", R, body={"calls": [{"func": "sum", "column": "金额", "criteria": [{"column": "已退款", "value": "TRUE"}]}]})
check("DSUMIFS", s == 200 and abs(r["data"]["values"][0] - sum(x[2] for x in ROWS if x[5])) < 0.01, (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/calc", R, body={"name": "单价", "formula": "=[金额]/[件数]"})
check("可以加计算列", s == 200, (s, r))
s, r = call("POST", "/ledger/sql/run", R, body={"sql": "SELECT count(*) AS n, sum(件数) AS q FROM 外部订单 WHERE 店铺 = '店A'"})
shop_a = [x for x in ROWS if x[1] == "店A"]
check("SQL 工作台可以查询（隐藏列不暴露）", s == 200 and r["data"]["rows"][0] == [len(shop_a), sum(x[3] for x in shop_a)], (s, r))
s, r = call("POST", "/ledger/sql/run", R, body={"sql": "SELECT __fi FROM 外部订单"})
check("隐藏列在 SQL 里不可见", s == 400, (s, r))

# ── 只读 ──
s, r = call("POST", f"/ledger/datasets/{D}/writeback", R, body={"mode": "append", "columns": ["单号"], "rows": [[1]]})
check("不能写入", s == 400 and "只读" in msg((s, r)), (s, r))

# ── 刷新：新文件、新列 ──
shutil.copy(os.path.join(FIX, "orders-2026-11.parquet"), DIR)
s, r = call("POST", f"/ledger/datasets/{D}/external/refresh", A)
check("普通用户不能刷新", s == 403, (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/external/refresh", R)
check("刷新：3 个文件、1510 行、多了「备注」列", s == 200 and r["data"]["fileCount"] == 3 and r["data"]["rows"] == 1510 and r["data"]["addedFiles"] == 1 and r["data"]["addedColumns"] == ["备注"], (s, r))
c3 = cur(spec())
check("刷新后旧游标作废，新游标看到新文件", c3["total"] == 1510 and rows(c3, 1505, 10)[-1][0] == 1509, c3)

# ── 文件被删掉：说清楚原因 ──
os.remove(os.path.join(DIR, "orders-2026-10.parquet"))
s, r = call("POST", f"/ledger/datasets/{D}/pivot", R, body={"rows": [{"column": "店铺"}], "values": [{"column": "件数", "agg": "sum"}]})
check("文件不在了：提示刷新文件列表", s == 400 and "刷新文件列表" in msg((s, r)), (s, r))
s, r = call("GET", "/ledger/datasets", R)
check("文件不在也能列出数据集", s == 200 and any(d["id"] == D for d in r["data"]), (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/external/refresh", R)
check("刷新后恢复", s == 200 and r["data"]["rows"] == 1010 and r["data"]["removedFiles"] == 1, (s, r))

# ── 删除进回收站 ──
s, r = call("DELETE", f"/ledger/datasets/{D}", R)
check("删除外部数据集（文件不动）", s == 200 and os.path.exists(os.path.join(DIR, "orders-2026-09.parquet")), (s, r))
finish()
