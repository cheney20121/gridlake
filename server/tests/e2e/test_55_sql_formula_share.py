"""单元格里的 SQL 公式与分享链接：访客打开的工作簿里有 =SQL() / =SQLVALUE()，能照常算出来，
但访客的 SQL 只能查这个工作簿用到的数据集（包括 SQL 公式里写到的），别的数据集一律查不到；数据范围跟随创建人。
"""
from lib import *  # noqa: F403

for t in (A, R): call("GET", "/ledger/me", t)


def mk(name, cols, rows, pk):
    s, r = call("POST", "/ledger/datasets/input", A, body={"name": name, "columns": [{"title": c, "kind": k} for c, k in cols], "primaryKey": [pk]})
    assert s == 200, (name, s, r)
    s2, r2 = call("POST", f"/ledger/datasets/{r['data']}/writeback", A, body={"mode": "upsert", "columns": [c for c, _ in cols], "rows": rows})
    assert s2 == 200, (name, s2, r2)
    return r["data"]


mk("订单", [("单号", "text"), ("店铺", "text"), ("金额", "money")], [["1", "A", 10], ["2", "A", 20], ["3", "B", 100]], "单号")
mk("成本", [("店铺", "text"), ("成本", "money")], [["A", 1], ["B", 2]], "店铺")
mk("机密", [("店铺", "text"), ("金额", "money")], [["A", 7]], "店铺")


def workbook(name, formulas):
    cells = {str(i): {"0": {"f": f}} for i, f in enumerate(formulas)}
    wb = {"sheetOrder": ["s1"], "sheets": {"s1": {"id": "s1", "name": "S", "cellData": cells}}, "resources": []}
    s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote(name), A, raw=gzip.compress(json.dumps(wb).encode()))
    assert s == 200, (s, r)
    return r["data"]["id"]


def share(wb):
    s, r = call("POST", f"/ledger/workbooks/{wb}/shares", A, body={"days": 7})
    assert s == 200, (s, r)
    tok = r["data"]["token"]
    s, r = call("GET", f"/ledger/workbooks/{wb}/content", None, headers={"x-share-token": tok}, binary=True)
    assert s == 200, (s, r)  # 打开分享页：按内容算出用到的数据集
    return tok


def guest_sql(tok, sql, params=None, values=None):
    return call("POST", "/ledger/sql/run", None, body={"sql": sql, "params": params or [], "values": values or {}}, headers={"x-share-token": tok})


WB = workbook("SQL 看板", [
    '=SQL("SELECT o.店铺, sum(金额 - 成本) AS 毛利 FROM 订单 o JOIN 成本 c USING (店铺) GROUP BY 1 ORDER BY 1")',
    '=SQLVALUE("SELECT count(*) FROM 订单 WHERE 店铺 = {{店铺}}", "店铺", "A")',
])
TOK = share(WB)
s, r = guest_sql(TOK, "SELECT o.店铺, sum(金额 - 成本) AS 毛利 FROM 订单 o JOIN 成本 c USING (店铺) GROUP BY 1 ORDER BY 1")
check("访客：工作簿里的 SQL 公式照常算（关联的两个数据集都在范围内）", s == 200 and r["data"]["rows"] == [["A", 28.0], ["B", 98.0]], (s, r))
s, r = guest_sql(TOK, "SELECT count(*) FROM 订单 WHERE 店铺 = {{店铺}}", [{"name": "店铺", "kind": "text"}], {"店铺": "A"})
check("访客：带参数的 SQLVALUE", s == 200 and r["data"]["rows"] == [[2]], (s, r))
s, r = guest_sql(TOK, "SELECT * FROM 机密")
check("访客：工作簿没用到的数据集查不到", s == 400 and "机密" in msg((s, r)), (s, r))
s, r = guest_sql(TOK, "DELETE FROM 订单")
check("访客：照样只能只读查询", s == 400, (s, r))
s, r = call("GET", "/ledger/datasets", None, headers={"x-share-token": TOK})
check("访客能看到的数据集：订单、成本（SQL 公式里写到的）", s == 200 and sorted(d["name"] for d in r["data"]) == ["成本", "订单"], (s, r))

# 没有 SQL 公式的工作簿：分享出去后访客的 SQL 什么也查不到
TOK2 = share(workbook("空白", ["=1+1"]))
s, r = guest_sql(TOK2, "SELECT count(*) FROM 订单")
check("没有 SQL 公式的工作簿：访客查不到任何数据集", s == 400, (s, r))

# 登录用户照常（不受分享范围限制）
s, r = call("POST", "/ledger/sql/run", A, body={"sql": "SELECT count(*) FROM 机密"})
check("登录用户的 SQL 不受影响", s == 200 and r["data"]["rows"] == [[1]], (s, r))
finish()
