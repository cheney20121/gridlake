"""N3 数据范围（按店铺的行级权限）

受限用户看到的数据集 / 游标 / 透视 / 公式下推 / SQL / 导出都只含自己店铺；游标缓存不串；受限操作 403；月结按店铺；规则即时生效；关联视图沿用左表过滤。
"""
from lib import *  # noqa: F403
for t in (A, Bo, C, R): call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}

# 数据：Alice 导入「订单」「成本」
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
orders = import_ds(A, "订单", "店铺,金额\nA,10\nA,20\nB,100\nB,200\nC,1000\nD,5000\n")
costs = import_ds(A, "成本", "店铺,成本\nA,1\nB,2\nC,3\n")
secret = import_ds(A, "机密", "店铺,金额\nA,7\nC,9\n")

# 管理员设置
s, r = call("PUT", "/ledger/data-scope/rules", Bo, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["A", "B"]}); check("非管理员不能设置范围", s == 403, (s, r))
s, r = call("GET", "/ledger/data-scope", R); check("管理员看到概览", s == 200 and {d["name"]: d["effective"] for d in r["data"]["datasets"]}.get("订单") == "hidden" and {d["name"]: d["effective"] for d in r["data"]["datasets"]}.get("资金流水") == "column", r)
s, r = call("PUT", f"/ledger/data-scope/datasets/{orders}", R, body={"scopeColumn": "店铺", "open": False}); check("订单按店铺列过滤", s == 200, (s, r))
s, r = call("PUT", f"/ledger/data-scope/datasets/{costs}", R, body={"open": True}); check("成本设为所有人可见", s == 200, (s, r))
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["A", "B"]}); check("给 Bob 设置范围 A、B", s == 200, (s, r))

# Alice（无规则）不受限，先建缓存
spec = {"filters": [], "keyword": None, "sort": [], "rule": None}
s, r = call("POST", f"/ledger/datasets/{orders}/cursor", A, body=spec); ca = r["data"]; check("Alice 游标 6 行", ca["total"] == 6, ca)

# Bob 视角
s, r = call("GET", "/ledger/me", Bo); check("me 带数据范围", r["data"]["scope"]["restricted"] and r["data"]["scope"]["shops"] == ["A", "B"], r["data"])
s, r = call("GET", "/ledger/datasets", Bo); names = {d["name"]: d["rowCount"] for d in r["data"]}
check("Bob 看到订单(4)与成本(3)与流水，看不到机密", names.get("订单") == 4 and names.get("成本") == 3 and "机密" not in names and "资金流水" in names, names)
s, r = call("GET", "/ledger/datasets", A); check("Alice 看到全部 4 个，订单 6 行", {d["name"]: d["rowCount"] for d in r["data"]}.get("订单") == 6 and len(r["data"]) == 4, r["data"])
s, r = call("POST", f"/ledger/datasets/{orders}/cursor", Bo, body=spec); cb = r["data"]
check("游标缓存不串：Bob 同样的查询只有 4 行", cb["total"] == 4 and cb["cursorId"] != ca["cursorId"], (ca, cb))
s, r = call("GET", f"/ledger/datasets/{orders}/rows?cursor={cb['cursorId']}&offset=0&limit=50", Bo)
vals = sorted((x[0], x[1]) for x in r["data"]["rows"]); check("Bob 取行只有 A、B", vals == [("A", 10.0), ("A", 20.0), ("B", 100.0), ("B", 200.0)], vals)
s, r = call("GET", f"/ledger/datasets/{orders}/rows?cursor={ca['cursorId']}&offset=0&limit=50", Bo)
vals = sorted(x[0] for x in r["data"]["rows"]); check("Bob 拿 Alice 的游标 ID 取行也只有 A、B", s == 200 and set(vals) <= {"A", "B"} and len(vals) == 4, (s, vals))
s, r = call("POST", f"/ledger/datasets/{orders}/distinct", Bo, body={**spec, "column": "店铺"}); dv = sorted(v["value"] for v in r["data"]["values"]) if s == 200 else r
check("列筛选候选值只有 A、B", dv == ["A", "B"], dv)
s, r = call("POST", f"/ledger/datasets/{orders}/stats", Bo, body={"cursorId": cb["cursorId"], "from": 0, "to": 3, "columns": ["金额"]}); print("   stats:", json.dumps(r.get("data"), ensure_ascii=False)[:160] if isinstance(r, dict) else r)
s, r = call("POST", f"/ledger/datasets/{orders}/pivot", Bo, body={"rows": [{"column": "店铺"}], "values": [{"column": "金额", "agg": "sum"}]})
d = r.get("data") if isinstance(r, dict) else {}
data_rows = [(x["keys"][0], x["cells"][0][0]) for x in d.get("rows", []) if x.get("kind") == "data"]
total = [x["cells"][0][0] for x in d.get("rows", []) if x.get("kind") != "data"]
check("透视只有 A、B，合计 330", s == 200 and data_rows == [("A", 30.0), ("B", 300.0)] and total == [330.0], (data_rows, total))
s, r = call("POST", f"/ledger/datasets/{orders}/aggregate", Bo, body={"calls": [{"func": "sum", "column": "金额", "criteria": []}, {"func": "sum", "column": "金额", "criteria": [{"column": "店铺", "value": "C"}]}]})
res = r["data"]; print("   aggregate:", json.dumps(res, ensure_ascii=False)[:200])
check("公式下推：总和 330，C 店为 0", s == 200 and json.dumps(res).count("330") >= 1 and "6330" not in json.dumps(res) and "1000" not in json.dumps(res), res)
s, r = call("GET", f"/ledger/datasets/{orders}/export?cursor={cb['cursorId']}&format=csv", Bo, binary=True)
txt = r.decode("utf-8-sig") if isinstance(r, bytes) else str(r); check("导出 CSV 只含 A、B", s == 200 and "C" not in txt.replace("店铺", "") and "D" not in txt and "A" in txt, txt)
s, r = call("POST", "/ledger/sql/run", Bo, body={"sql": "SELECT 店铺, sum(金额) AS 合计 FROM 订单 GROUP BY 1 ORDER BY 1"}); rows = r["data"]["rows"] if s == 200 else r
check("SQL 只有 A、B", s == 200 and [x[0] for x in rows] == ["A", "B"], rows)
s, r = call("POST", "/ledger/sql/run", Bo, body={"sql": "SELECT * FROM 机密"}); check("SQL 查不到未绑定的数据集", s == 400 and ("不存在" in msg((s, r)) or "没有" in msg((s, r))), (s, r))
s, r = call("POST", "/ledger/sql/run", Bo, body={"sql": "SELECT o.店铺, o.金额, c.成本 FROM 订单 o JOIN 成本 c ON o.店铺 = c.店铺 ORDER BY 1, 2"}); check("SQL 跨数据集关联：订单受限、成本全开", s == 200 and len(r["data"]["rows"]) == 4, r)
s, r = call("POST", f"/ledger/datasets/{secret}/cursor", Bo, body=spec); check("直接访问未绑定数据集 400", s == 400, (s, r))
s, r = call("GET", f"/ledger/datasets/{secret}/export?cursor=x&format=csv", Bo); check("导出未绑定数据集失败", s >= 400, s)

# Bob 被拒的操作
for label, m, p, b in [("导入预览", "POST", "/ledger/datasets/import/preview", None), ("建关联视图", "POST", "/ledger/datasets/views", {"name": "v"}), ("改数据集名", "PUT", f"/ledger/datasets/{orders}", {"name": "x"}),
                       ("保存 SQL", "POST", "/ledger/sql/queries", {"name": "q", "sql": "select 1"}), ("定时推送列表", "GET", "/ledger/reports", None), ("撤销填报", "POST", "/ledger/writebacks/wb_x/undo", {}), ("填报记录", "GET", "/ledger/writebacks", None),
                       ("台账导入", "POST", "/ledger/imports/preview", None), ("台账余额", "GET", "/ledger/balances?period=2026-09", None), ("数据连接列表", "GET", "/ledger/connectors", None), ("数据集使用情况", "GET", f"/ledger/datasets/{orders}/usage", None)]:
    s, r = call(m, p, Bo, body=b) if m != "POST" or b is not None else call(m, p, Bo, raw=b"x")
    check(f"受限用户不能：{label}", s == 403 and "完整的数据权限" in msg((s, r)), (s, msg((s, r))))
s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("Bob 看板"), Bo, raw=b"x"); check("受限用户仍可新建工作簿（内容校验失败不是 403）", s != 403, (s, r))

# 月结与店铺
call("POST", "/ledger/shops", A, body={"name": "A", "status": "active"}); call("POST", "/ledger/shops", A, body={"name": "C", "status": "active"})
s, r = call("GET", "/ledger/shops", Bo); check("店铺列表只有 A", [x["name"] for x in r["data"]] == ["A"], r)
for sh in ("A", "C"):
    call("POST", "/ledger/close", A, body={"shop": sh, "period": "2026-09", "action": "submit", "approverId": ids["Bob"], "comment": "x", "acceptRisk": True})
s, r = call("GET", "/ledger/close?period=2026-09", Bo); check("Bob 月结总览只有 A，且能审批", [x["shop"] for x in r["data"]["shops"]] == ["A"] and r["data"]["shops"][0]["can"]["decide"], r)
s, r = call("GET", "/ledger/close?period=2026-09", A); check("Alice 看到 A、C", sorted(x["shop"] for x in r["data"]["shops"]) == ["A", "C"], r)
s, r = call("POST", "/ledger/close", Bo, body={"shop": "C", "period": "2026-09", "action": "approve", "comment": ""}); check("Bob 不能审批范围外的 C 店", s == 403 and "不在你的数据范围" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/close", Bo, body={"shop": "A", "period": "2026-09", "action": "approve", "comment": "ok"}); check("Bob 可以审批范围内的 A 店", s == 200, (s, r))
s, r = call("GET", "/ledger/close/locks", Bo); check("锁账列表只有 A", [x["shop"] for x in r["data"]] == ["A"], r)
s, r = call("GET", "/ledger/close/logs?period=2026-09", Bo); check("月结日志只有 A", {x["shop"] for x in r["data"]} == {"A"}, r)

# 规则生效即时、默认规则、删除
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["C"]})
s, r = call("GET", "/ledger/datasets", Bo); check("改规则立即生效：Bob 现在看到 C 店 1 行", {d["name"]: d["rowCount"] for d in r["data"]}.get("订单") == 1, r["data"])
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": "*", "mode": "shops", "shops": ["D"]})
s, r = call("GET", "/ledger/datasets", C); check("默认规则：Carol 只看到 D 店 1 行", {d["name"]: d["rowCount"] for d in r["data"]}.get("订单") == 1, r["data"])
s, r = call("GET", "/ledger/datasets", A); check("Alice 受默认规则限制了(她没有单独规则)", {d["name"]: d["rowCount"] for d in r["data"]}.get("订单") == 1, r["data"])
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Alice"], "mode": "all", "shops": []})
s, r = call("GET", "/ledger/datasets", A); check("给 Alice 单独设置「不限」后恢复全部", {d["name"]: d["rowCount"] for d in r["data"]}.get("订单") == 6, r["data"])
s, r = call("GET", "/ledger/datasets", R); check("管理员始终全部", {d["name"]: d["rowCount"] for d in r["data"]}.get("订单") == 6 and len(r["data"]) == 4, r["data"])
s, r = call("DELETE", f"/ledger/data-scope/rules/*", R); s, r = call("GET", "/ledger/datasets", C); check("删除默认规则后 Carol 恢复全部", {d["name"]: d["rowCount"] for d in r["data"]}.get("订单") == 6, r["data"])
time.sleep(3)
s, r = call("GET", "/ledger/audit?size=200&action=" + urllib.parse.quote("范围"), R); acts = [(i["userName"], i["action"], i["status"]) for i in r["data"]["items"]]
check("审计含范围设置与被拒绝的尝试", ("Root", "设置用户数据范围", 200) in acts and ("Bob", "设置用户数据范围", 403) in acts, acts[:5])

# ── 关联视图沿用左表的过滤列 ──
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}
ds = {d["name"]: d["id"] for d in call("GET", "/ledger/datasets", R)[1]["data"]}
call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["A", "B"]})
s, r = call("GET", "/ledger/datasets/views/preview" if False else "/ledger/datasets", A)
s, r = call("POST", "/ledger/datasets/views", A, body={"name": "订单成本", "left": ds["订单"], "right": ds["成本"], "on": [{"left": "店铺", "right": "店铺"}], "columns": [{"source": "成本", "title": "成本价"}]})
check("Alice 建关联视图", s == 200, (s, r)); vid = r["data"]
s, r = call("GET", "/ledger/datasets", Bo); v = [d for d in r["data"] if d["id"] == vid]
check("未单独设置的视图沿用左表过滤列：Bob 看到 4 行", v and v[0]["rowCount"] == 4, [(d["name"], d["rowCount"]) for d in r["data"]])
s, r = call("GET", "/ledger/data-scope", R); b = [d for d in r["data"]["datasets"] if d["id"] == vid][0]
check("管理界面显示为「沿用左表」", b["effective"] == "inherit" and b["effectiveColumn"] == "店铺", b)
s, r = call("POST", f"/ledger/datasets/{vid}/cursor", Bo, body={"filters": [], "keyword": None, "sort": [], "rule": None}); cur = r["data"]
s, r = call("GET", f"/ledger/datasets/{vid}/rows?cursor={cur['cursorId']}&offset=0&limit=50", Bo)
shops = sorted(x[0] for x in r["data"]["rows"]); check("视图取行只有 A、B", shops == ["A", "A", "B", "B"], shops)
# 左表改为不绑定 → 视图对受限用户不可见
call("PUT", f"/ledger/data-scope/datasets/{ds['订单']}", R, body={"open": False})
s, r = call("GET", "/ledger/datasets", Bo); names = [d["name"] for d in r["data"]]
check("左表解除绑定后，订单与视图都对 Bob 隐藏", "订单" not in names and "订单成本" not in names, names)

finish()
