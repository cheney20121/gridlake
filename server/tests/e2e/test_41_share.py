"""只读分享链接：创建 / 吊销 / 过期，访客只能读被分享的工作簿和它用到的数据集，数据范围跟随创建人

场景：Alice 有一个看板（图表用「订单」，公式里按名字用「成本」），库里还有不相干的「机密」。
"""
import datetime

from lib import *  # noqa: F403

for t in (A, Bo, C, R): call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}


def mk(tok, name, cols, rows, pk):
    s, r = call("POST", "/ledger/datasets/input", tok, body={"name": name, "columns": [{"title": c, "kind": k} for c, k in cols], "primaryKey": [pk]})
    assert s == 200, (name, s, r)
    ds = r["data"]
    s, r = call("POST", f"/ledger/datasets/{ds}/writeback", tok, body={"mode": "upsert", "columns": [c for c, _ in cols], "rows": rows})
    assert s == 200, (name, s, r)
    return ds


orders = mk(A, "订单", [("单号", "text"), ("店铺", "text"), ("金额", "money")], [["1", "A", 10], ["2", "A", 20], ["3", "B", 100], ["4", "B", 200], ["5", "C", 1000]], "单号")
costs = mk(A, "成本", [("店铺", "text"), ("成本", "money")], [["A", 1], ["B", 2]], "店铺")
secret = mk(A, "机密", [("店铺", "text"), ("金额", "money")], [["A", 7], ["C", 9]], "店铺")


def workbook(tok, name, dataset_id=None, formula_dataset=None):
    cells = {"0": {"0": {"v": "看板"}}}
    if formula_dataset:
        cells["1"] = {"0": {"f": f'=DSUMIFS("{formula_dataset}","成本")', "v": 3}}
    wb = {"sheetOrder": ["s1"], "sheets": {"s1": {"id": "s1", "name": "S", "cellData": cells}}, "resources": []}
    if dataset_id:
        drawings = {"s1": {"data": {"d1": {"componentKey": "webexcel-kpi", "data": {"datasetId": dataset_id, "value": {"column": "金额", "agg": "sum"}}}}}}
        wb["resources"].append({"name": "SHEET_DRAWING_PLUGIN", "data": json.dumps(drawings)})
    s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote(name), tok, raw=gzip.compress(json.dumps(wb).encode()))
    assert s == 200, (s, r)
    return r["data"]["id"], wb


WB, WBJ = workbook(A, "销售看板", dataset_id=orders, formula_dataset="成本")
OTHER, _ = workbook(A, "别的工作簿")

# ── 创建与权限 ──
s, r = call("POST", f"/ledger/workbooks/{WB}/shares", Bo, body={"note": "x"}); check("没有权限的人不能创建分享链接", s == 403, (s, r))
s, r = call("POST", f"/ledger/workbooks/{WB}/shares", A, body={"note": "x", "days": 0}); check("创建（永不过期）", s == 200 and r["data"]["token"].startswith("gls_") and r["data"]["expiresAt"] is None, (s, r))
s, r = call("POST", f"/ledger/workbooks/{WB}/shares", A, body={"note": "给客户", "days": 7}); check("创建（7 天）", s == 200 and r["data"]["expiresAt"] and r["data"]["note"] == "给客户", (s, r))
made = r["data"]; TOK = made["token"]; SID = made["id"]
check("响应不含摘要", "tokenHash" not in made and made["prefix"] == TOK[:8], made)
s, r = call("POST", f"/ledger/workbooks/{WB}/shares", A, body={"days": 9999}); check("有效期过长被拒", s == 400, (s, r))
s, r = call("POST", f"/ledger/workbooks/{WB}/shares", A, body={"note": "x" * 61}); check("备注过长被拒", s == 400, (s, r))
s, r = call("GET", f"/ledger/workbooks/{WB}/shares", A); check("列表有 2 条且没有令牌", s == 200 and len(r["data"]) == 2 and all("token" not in x for x in r["data"]), (s, r))
s, r = call("GET", f"/ledger/workbooks/{WB}/shares", Bo); check("别人看不到分享列表", s == 403, (s, r))
s, r = call("DELETE", f"/ledger/shares/{SID}", Bo); check("别人不能吊销", s == 403, (s, r))

# ── 访客：只靠令牌 ──
V = {"x-share-token": TOK}
vcall = lambda m, p, body=None, tok=TOK: call(m, p, None, body=body, headers={"x-share-token": tok})
s, r = vcall("GET", "/ledger/me")
check("访客 /me：share 模式，带被分享的工作簿", s == 200 and r["data"]["mode"] == "share" and r["data"]["share"]["workbookId"] == WB and r["data"]["share"]["workbookName"] == "销售看板", (s, r))
s, r = vcall("GET", f"/ledger/workbooks/{WB}")
check("访客读元信息，不含所有者", s == 200 and r["data"]["id"] == WB and r["data"]["ownerId"] is None and r["data"]["ownerName"] is None, (s, r))
s, r = vcall("GET", f"/ledger/workbooks/{OTHER}"); check("访客读不了别的工作簿", s == 403, (s, r))
s, r = vcall("GET", f"/ledger/workbooks/{OTHER}/content"); check("访客读不了别的工作簿内容", s == 403, (s, r))
spec = {"filters": [], "keyword": None, "sort": [], "rule": None}
s, r = vcall("POST", f"/ledger/datasets/{orders}/cursor", spec); check("还没打开分享页（读内容）前，数据集都不开放", s == 403, (s, r))
s, raw = call("GET", f"/ledger/workbooks/{WB}/content", None, headers=V, binary=True)
got = json.loads(gzip.decompress(raw)) if s == 200 else None
check("访客读工作簿内容", s == 200 and got and got["sheets"]["s1"]["cellData"]["0"]["0"]["v"] == "看板", (s, raw[:80] if isinstance(raw, bytes) else raw))
s, r = vcall("GET", "/ledger/datasets")
check("数据集列表只含这个工作簿用到的（图表用的「订单」、公式里写的「成本」）", s == 200 and sorted(d["name"] for d in r["data"]) == ["成本", "订单"], (s, r))
s, r = vcall("POST", f"/ledger/datasets/{orders}/cursor", spec); cur = r["data"] if s == 200 else r
check("访客能打开被分享数据集的游标", s == 200 and cur["total"] == 5, (s, r))
s, r = vcall("GET", f"/ledger/datasets/{orders}/rows?cursor={cur['cursorId']}&offset=0&limit=10"); check("访客能取行", s == 200 and len(r["data"]["rows"]) == 5, (s, r))
s, r = vcall("POST", f"/ledger/datasets/{orders}/pivot", {"rows": [{"column": "店铺"}], "values": [{"column": "金额", "agg": "sum"}]})
check("访客能做透视", s == 200 and any(x.get("kind") == "total" and x["cells"][0][0] == 1330 for x in r["data"]["rows"]), (s, r))
s, r = vcall("POST", f"/ledger/datasets/{costs}/aggregate", {"calls": [{"func": "sum", "column": "成本", "criteria": []}]}); check("访客能做公式下推", s == 200 and r["data"]["values"] == [3], (s, r))
s, r = vcall("POST", "/ledger/datasets/" + urllib.parse.quote("成本") + "/aggregate", {"calls": [{"func": "max", "column": "成本", "criteria": []}]}); check("公式下推按数据集名称请求也行（被引用的数据集）", s == 200 and r["data"]["values"] == [2], (s, r))
s, r = vcall("POST", "/ledger/datasets/" + urllib.parse.quote("机密") + "/aggregate", {"calls": [{"func": "sum", "column": "金额", "criteria": []}]}); check("按名称也不能读没被引用的数据集", s == 403, (s, r))
s, r = vcall("POST", f"/ledger/datasets/{secret}/cursor", spec); check("没被引用的「机密」不开放", s == 403 and "不在被分享" in msg((s, r)), (s, r))
s, r = vcall("POST", f"/ledger/datasets/{secret}/pivot", {"rows": [], "values": [{"column": "金额", "agg": "sum"}]}); check("「机密」的透视也不行", s == 403, (s, r))

# 只读：写、导出、SQL、AI、评论、协作、别的列表……一律拒绝
denied = [
    ("PUT", f"/ledger/workbooks/{WB}/content?baseVersion=1", None), ("DELETE", f"/ledger/workbooks/{WB}", None), ("PUT", f"/ledger/workbooks/{WB}", {"name": "x"}),
    ("POST", f"/ledger/datasets/{orders}/writeback", {"mode": "upsert", "columns": ["单号", "店铺", "金额"], "rows": [["9", "A", 1]]}),
    ("GET", f"/ledger/datasets/{orders}/export?cursor={cur['cursorId']}&format=csv", None),
    ("POST", "/ledger/ai/threads", {"workbookId": WB}),
    ("GET", f"/ledger/workbooks/{WB}/comments", None), ("GET", f"/ledger/collab/{WB}/events", None), ("GET", "/ledger/workbooks", None),
    ("GET", f"/ledger/workbooks/{WB}/versions", None), ("GET", f"/ledger/workbooks/{WB}/shares", None), ("POST", f"/ledger/workbooks/{WB}/shares", {}),
    ("DELETE", f"/ledger/shares/{SID}", None), ("GET", "/ledger/audit", None), ("GET", "/ledger/tokens", None), ("GET", "/ledger/accounts", None),
    ("POST", "/ledger/workbooks/ranges", {}), ("PUT", f"/ledger/workbooks/{WB}/share", {"visibility": "edit"}),
]
bad = [(m, p) for m, p, b in denied if vcall(m, p, b)[0] != 403]
check("访客的写入 / 导出 / AI / 评论 / 协作 / 管理接口全部 403", not bad, bad)
# SQL 接口只给单元格里的 SQL 公式用：只能查这个工作簿用到的数据集（这里是「订单」「成本」），查不到「机密」
s, r = vcall("POST", "/ledger/sql/run", {"sql": "SELECT * FROM 机密"})
check("访客的 SQL 查不到工作簿没用到的数据集", s == 400 and "机密" in msg((s, r)), (s, r))

# ── 无效令牌 ──
for label, tok in [("随便编的令牌", "gls_" + "x" * 43), ("不是分享令牌", "glp_abc"), ("把登录令牌放进分享头", A)]:
    s, r = vcall("GET", "/ledger/me", tok=tok); check(f"无效：{label}", s == 403 and "无效或已失效" in msg((s, r)), (s, r))
s, r = call("GET", "/ledger/me", None, headers={"x-share-token": ""}); check("空的分享头按没登录处理", s == 200 and r["data"]["user"] is None, (s, r))

# ── 吊销、过期、回收站 ──
s, r = call("DELETE", f"/ledger/shares/{SID}", A); check("创建人吊销", s == 200, (s, r))
s, r = vcall("GET", "/ledger/me"); check("吊销后立即失效", s == 403, (s, r))
s, r = call("GET", f"/ledger/workbooks/{WB}/shares", A); check("吊销后列表少一条", len(r["data"]) == 1, r)
FOREVER = call("GET", f"/ledger/workbooks/{WB}/shares", A)[1]["data"][0]["id"]
s, r = call("POST", f"/ledger/workbooks/{WB}/shares", R, body={"note": "管理员建的", "days": 1}); check("管理员也能给别人的工作簿建链接", s == 200, (s, r))
RT = r["data"]["token"]; RID = r["data"]["id"]
s, r = vcall("GET", "/ledger/me", tok=RT); check("管理员建的链接可用", s == 200, (s, r))
s, r = call("DELETE", f"/ledger/workbooks/{WB}", A); check("工作簿进回收站", s == 200, (s, r))
s, r = vcall("GET", "/ledger/me", tok=RT); check("工作簿在回收站时链接失效", s == 403, (s, r))
s, r = call("POST", f"/ledger/workbooks/{WB}/untrash", A); check("恢复工作簿", s == 200, (s, r))
s, r = vcall("GET", "/ledger/me", tok=RT); check("恢复后链接又可用", s == 200, (s, r))
call("DELETE", f"/ledger/shares/{RID}", R)

# ── 数据范围跟随创建人 ──
s, r = call("PUT", f"/ledger/data-scope/datasets/{orders}", R, body={"scopeColumn": "店铺", "open": False}); check("订单按店铺列过滤", s == 200, (s, r))
s, r = call("PUT", f"/ledger/data-scope/datasets/{costs}", R, body={"open": True})
CWB, _ = workbook(C, "Carol 的看板", dataset_id=orders)
s, r = call("POST", f"/ledger/workbooks/{CWB}/shares", C, body={"note": "c", "days": 5}); check("不受限的 Carol 能创建分享链接", s == 200, (s, r))
CT = r["data"]["token"]
call("GET", f"/ledger/workbooks/{CWB}/content", None, headers={"x-share-token": CT}, binary=True)
pivot = {"rows": [{"column": "店铺"}], "values": [{"column": "金额", "agg": "sum"}]}
total = lambda r: next((x["cells"][0][0] for x in r["data"]["rows"] if x.get("kind") != "data"), None)
s, r = vcall("POST", f"/ledger/datasets/{orders}/pivot", pivot, tok=CT); check("Carol 不受限：访客看到全部 1330", s == 200 and total(r) == 1330, (s, r))
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Carol"], "mode": "shops", "shops": ["A"]}); check("管理员把 Carol 限制到 A 店", s == 200, (s, r))
s, r = vcall("POST", f"/ledger/datasets/{orders}/pivot", pivot, tok=CT); check("创建人被限制后，访客也只看到 A 店（30）", s == 200 and total(r) == 30, (s, r))
s, r = vcall("POST", f"/ledger/datasets/{orders}/cursor", spec, tok=CT); check("游标也只有 A 店 2 行", s == 200 and r["data"]["total"] == 2, (s, r))
s, r = call("POST", f"/ledger/workbooks/{CWB}/shares", C, body={"note": "c"}); check("受限用户不能再创建分享链接", s == 403 and "完整的数据权限" in msg((s, r)), (s, r))

# ── 访问密码 ──
s, r = call("POST", f"/ledger/workbooks/{WB}/shares", A, body={"note": "短密码", "password": "12345"}); check("密码太短被拒", s == 400 and "至少 6 位" in msg((s, r)), (s, r))
s, r = call("POST", f"/ledger/workbooks/{WB}/shares", A, body={"note": "太长", "password": "x" * 129}); check("密码太长被拒", s == 400, (s, r))
s, r = call("POST", f"/ledger/workbooks/{WB}/shares", A, body={"note": "带密码", "days": 5, "password": "open-sesame"})
check("创建带密码的链接：列表里标出受保护，响应里没有密码或摘要", s == 200 and r["data"]["protected"] is True and "open-sesame" not in json.dumps(r) and "passwordHash" not in r["data"], (s, r))
PT, PID = r["data"]["token"], r["data"]["id"]
s, r = call("GET", f"/ledger/workbooks/{WB}/shares", A); check("列表里普通链接 protected=false", {x["id"]: x["protected"] for x in r["data"]}[PID] is True and sum(1 for x in r["data"] if not x["protected"]) >= 1, r)
unlock = lambda tok, pw: call("POST", "/ledger/share/unlock", None, body={"token": tok, "password": pw})
s, r = vcall("GET", "/ledger/me", tok=PT)
check("没输密码：/me 只说需要密码，不透露工作簿名称", s == 200 and r["data"]["mode"] == "share" and r["data"]["share"] == {"workbookId": "", "workbookName": "", "passwordRequired": True}, (s, r))
for label, m, pth, b in [("工作簿元信息", "GET", f"/ledger/workbooks/{WB}", None), ("工作簿内容", "GET", f"/ledger/workbooks/{WB}/content", None), ("数据集列表", "GET", "/ledger/datasets", None), ("数据集游标", "POST", f"/ledger/datasets/{orders}/cursor", spec)]:
    s, r = vcall(m, pth, b, tok=PT); check(f"没输密码：{label}被拒", s == 403 and "需要访问密码" in msg((s, r)), (s, r))
s, r = call("GET", f"/ledger/workbooks/{WB}/content", None, headers={"x-share-token": PT, "x-share-proof": "0" * 64}, binary=True); check("随便编的通行证无效", s == 403, s)
s, r = call("GET", f"/ledger/workbooks/{WB}/content", None, headers={"x-share-token": PT, "x-share-proof": PT}, binary=True); check("拿令牌当通行证也无效", s == 403, s)
s, r = unlock(PT, "wrong-pass"); check("密码错：被拒绝", s == 400 and msg((s, r)) == "访问密码不正确", (s, r))
s, r = unlock("gls_" + "x" * 43, "open-sesame"); check("令牌无效：说明链接失效而不是密码问题", s == 403 and "无效或已失效" in msg((s, r)), (s, r))
s, r = unlock(TOK, "open-sesame"); check("没设密码的链接不需要解锁", s in (400, 403), (s, r))
s, r = unlock(PT, "open-sesame"); PROOF = r["data"]["proof"] if s == 200 else ""
check("密码对：拿到通行证（64 位十六进制）", s == 200 and len(PROOF) == 64 and PROOF != PT, (s, r))
pv = lambda m, pth, body=None: call(m, pth, None, body=body, headers={"x-share-token": PT, "x-share-proof": PROOF})
s, r = pv("GET", "/ledger/me"); check("带通行证：/me 给出工作簿", s == 200 and r["data"]["share"] == {"workbookId": WB, "workbookName": "销售看板"}, (s, r))
s, raw = call("GET", f"/ledger/workbooks/{WB}/content", None, headers={"x-share-token": PT, "x-share-proof": PROOF}, binary=True); check("带通行证：能读内容", s == 200, s)
s, r = pv("POST", f"/ledger/datasets/{orders}/cursor", spec); check("带通行证：能查数据集", s == 200 and r["data"]["total"] == 5, (s, r))
s, r = pv("POST", "/ledger/sql/run", {"sql": "DELETE FROM 订单"}); check("带通行证也仍然只读（SQL 只能查询）", s == 400, (s, r))
s, r = call("POST", f"/ledger/workbooks/{WB}/shares", A, body={"note": "同密码", "password": "open-sesame"}); T2 = r["data"]["token"]
s, r = call("GET", f"/ledger/workbooks/{WB}/content", None, headers={"x-share-token": T2, "x-share-proof": PROOF}, binary=True); check("同一个密码的另一条链接：通行证不通用", s == 403, s)
for _ in range(5):
    unlock(T2, "bad-guess")
s, r = unlock(T2, "open-sesame"); check("连续输错 5 次后锁定：即使这次对了也要等", s == 400 and "次数过多" in msg((s, r)), (s, r))
s, r = unlock(PT, "open-sesame"); check("锁定按链接计：另一条链接不受影响", s == 200, (s, r))
s, r = call("DELETE", f"/ledger/shares/{PID}", A); check("吊销带密码的链接", s == 200, (s, r))
s, r = pv("GET", "/ledger/me"); check("吊销后通行证作废", s == 403 and "无效或已失效" in msg((s, r)), (s, r))

# ── 审计 ──
time.sleep(3)  # 审计按 2 秒一批异步写入
s, r = call("GET", "/ledger/audit?size=300", R)
actions = " ".join(x.get("action", "") for x in r["data"]["items"]) if s == 200 else str(r)
check("审计里有创建、吊销分享链接和打开分享页", all(k in actions for k in ("创建分享链接", "吊销分享链接", "打开分享链接", "分享链接密码验证通过", "分享链接密码验证失败")), actions[:300])
finish()
