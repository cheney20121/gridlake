"""参数化 SQL 查询：{{名称}} 占位按类型安全代入；同一个查询用不同的参数值生成多个实例数据集

随机造订单，用 Python 独立算出每组参数下的行数与合计，与服务端对照；注入式的值只会被当作普通文本；
缺值 / 类型不对 / 引号 / 未定义 / 未使用等都给出明确说明；实例各自独立重算。
"""
import random
from datetime import date, timedelta
from lib import *  # noqa: F403
for t in (A, Bo): call("GET", "/ledger/me", t)
time.sleep(1)

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
def upload(tok, name, csv, mode="create", ds=None):
    s, r = call("POST", "/ledger/datasets/import/preview", tok, raw=mp(name + ".csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
    assert s == 200, (s, r)
    p = r["data"]
    body = {"token": p["token"], "mode": mode, "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]}
    if mode == "create": body["name"] = name
    else: body["datasetId"] = ds
    s, r = call("POST", "/ledger/datasets/import/commit", tok, body=body)
    assert s == 200, (s, r)
    return r["data"]["datasetId"]

random.seed(15)
D0 = date(2026, 8, 1)
rows = [(random.choice(["A", "B", "C", "O'Neil"]), D0 + timedelta(days=random.randint(0, 60)), random.randint(1, 500)) for _ in range(400)]
csv = lambda rs: "店铺,日期,金额\n" + "\n".join(f'"{s}",{d.isoformat()},{a}' for s, d, a in rs) + "\n"
ORD = upload(A, "订单", csv(rows))
want = lambda shop, since, rs=rows: [(s, d, a) for s, d, a in rs if s == shop and d >= since]

SQL = "SELECT 日期, 金额 FROM 订单 WHERE 店铺 = {{店铺}} AND 日期 >= {{起}} ORDER BY 日期"
DEFS = [{"name": "店铺", "kind": "text", "choices": []}, {"name": "起", "kind": "date", "default": "2026-09-01"}]
norm = lambda ps: [{"name": p["name"], "kind": p["kind"], "default": p.get("default"), "choices": p.get("choices", [])} for p in ps]
run = lambda sql=SQL, params=DEFS, values=None, tok=A: call("POST", "/ledger/sql/run", tok, body={"sql": sql, "params": params, "values": values or {}})

# ── 1. 运行 ──
s, r = run(values={"店铺": "A"})
check("按参数运行：用默认的起始日期", s == 200 and len(r["data"]["rows"]) == len(want("A", date(2026, 9, 1))) and abs(sum(x[1] for x in r["data"]["rows"]) - sum(a for _, _, a in want("A", date(2026, 9, 1)))) < 0.01, (s, r))
s, r = run(values={"店铺": "B", "起": "2026-09-15"})
check("填的值覆盖默认值", s == 200 and len(r["data"]["rows"]) == len(want("B", date(2026, 9, 15))), (s, r))
s, r = run(values={"店铺": "O'Neil"})
check("值里有单引号：正常按文本处理", s == 200 and len(r["data"]["rows"]) == len(want("O'Neil", date(2026, 9, 1))) and len(r["data"]["rows"]) > 0, (s, r))
s, r = run(values={"店铺": "A' OR '1'='1"})
check("注入式的值只是普通文本：查不到行，也不报错", s == 200 and r["data"]["rows"] == [], (s, r))
s, r = run(values={"店铺": "A'; DROP TABLE x; --"})
check("带 DROP 的值同样无害", s == 200 and r["data"]["rows"] == [], (s, r))
s, r = call("POST", "/ledger/sql/run", A, body={"sql": "SELECT count(*) AS n FROM 订单"}); check("订单仍在", s == 200 and r["data"]["rows"][0][0] == 400, (s, r))

# ── 2. 错误说明 ──
s, r = run(values={}); check("缺值：说明是哪个参数", s == 400 and "请填写参数「店铺」" in msg((s, r)), (s, r))
s, r = run(values={"店铺": "A", "起": "9月1日"}); check("日期格式不对", s == 400 and "2026-09-01" in msg((s, r)), (s, r))
s, r = run(sql="SELECT 金额 FROM 订单 WHERE 金额 > {{下限}}", params=[{"name": "下限", "kind": "number"}], values={"下限": "abc"}); check("数字不对", s == 400 and "应为数字" in msg((s, r)), (s, r))
s, r = run(sql="SELECT 金额 FROM 订单 WHERE 金额 > {{下限}}", params=[{"name": "下限", "kind": "number"}], values={"下限": "450"})
check("数字参数", s == 200 and len(r["data"]["rows"]) == sum(1 for x in rows if x[2] > 450), (s, r))
s, r = run(sql="SELECT 1 FROM 订单 WHERE 店铺 = {{店铺}}", params=[], values={}); check("SQL 里有参数但没定义", s == 400 and "没有定义" in msg((s, r)), (s, r))
s, r = run(sql="SELECT 1 FROM 订单", params=[{"name": "x", "kind": "text"}], values={"x": "1"}); check("定义了但没用", s == 400 and "没有在 SQL 里使用" in msg((s, r)), (s, r))
s, r = run(sql="SELECT 1 FROM 订单 WHERE 店铺 = '{{店铺}}'", params=[DEFS[0]], values={"店铺": "A"}); check("自己加引号被拒", s == 400 and "不要自己加引号" in msg((s, r)), (s, r))
s, r = run(sql="SELECT 1 FROM 订单 WHERE 店铺 = {{店铺}}", params=[{"name": "店铺", "kind": "text", "choices": ["A", "B"]}], values={"店铺": "C"}); check("限定可选值", s == 400 and "只能取：A、B" in msg((s, r)), (s, r))
s, r = run(sql="DELETE FROM 订单 WHERE 店铺 = {{店铺}}", params=[DEFS[0]], values={"店铺": "A"}); check("参数化也过只读检查", s == 400, (s, r))

# ── 3. 保存与实例 ──
def save(name, **kw):
    return call("POST", "/ledger/sql/queries", A, body={"name": name, "sql": SQL, "params": DEFS, "schedule": {"kind": "manual"}, **kw})
s, r = save("分店订单"); check("保存参数化查询（不填值也行：只检查语法）", s == 200 and norm(r["data"]["params"]) == norm(DEFS) and r["data"]["datasetId"] is None, (s, r))
Q = r["data"]["id"]
s, r = call("POST", "/ledger/sql/queries", A, body={"name": "坏的", "sql": "SELECT {{x}} FROM 订单", "params": []}); check("保存时 SQL 里的参数必须有定义", s == 400, (s, r))
s, r = call("POST", f"/ledger/sql/queries/{Q}/refresh", A, body={}); check("模板查询本身没有数据集，不能重算", s == 400 and "还没有另存为数据集" in msg((s, r)), (s, r))

def instance(values, tok=A, **kw):
    return call("POST", f"/ledger/sql/queries/{Q}/instances", tok, body={"values": values, **kw})
s, r = instance({"店铺": "A"}); inst_a = r["data"]
check("生成实例 A：名称带参数、关联新数据集、记录模板", s == 200 and inst_a["name"] == "分店订单（店铺=A，起=2026-09-01）" and inst_a["datasetId"] and inst_a["templateId"] == Q and inst_a["values"] == {"店铺": "A"}, (s, r))
s, r = instance({"店铺": "B", "起": "2026-09-20"}, tok=Bo, name="B店九月下旬"); inst_b = r["data"]
check("别的用户也能生成实例，可自定名称", s == 200 and inst_b["name"] == "B店九月下旬", (s, r))
s, r = call("GET", "/ledger/datasets", A); ds = {d["name"]: d for d in r["data"]}
wa, wb_ = want("A", date(2026, 9, 1)), want("B", date(2026, 9, 20))
check("两个实例数据集各自的行数正确", ds["分店订单（店铺=A，起=2026-09-01）"]["rowCount"] == len(wa) and ds["B店九月下旬"]["rowCount"] == len(wb_), (len(wa), len(wb_), {k: v["rowCount"] for k, v in ds.items()}))
s, r = call("POST", f"/ledger/datasets/{inst_a['datasetId']}/aggregate", A, body={"calls": [{"func": "sum", "column": "金额", "criteria": []}]})
check("实例数据集的金额合计与独立算的一致", s == 200 and abs(r["data"]["values"][0] - sum(a for _, _, a in wa)) < 0.01, (s, r, sum(a for _, _, a in wa)))
s, r = instance({"店铺": "A"}); check("同样的参数再生成：数据集重名被拒", s == 400, (s, r))
s, r = instance({}); check("实例缺值", s == 400 and "请填写参数「店铺」" in msg((s, r)), (s, r))
s, r = instance({"店铺": "A", "起": "x"}, name="另一个"); check("实例值类型不对", s == 400 and "应为日期" in msg((s, r)), (s, r))
s, r = call("GET", "/ledger/datasets", A); check("失败的实例不留下数据集 / 查询", "另一个" not in [d["name"] for d in r["data"]])
s, r = call("GET", "/ledger/sql/queries", A); qs = {q["name"]: q for q in r["data"]}
check("清单里：模板与实例，实例带模板 id 与取值", norm(qs["分店订单"]["params"]) == norm(DEFS) and qs["B店九月下旬"]["templateId"] == Q and qs["B店九月下旬"]["values"]["起"] == "2026-09-20", list(qs))
s, r = call("POST", "/ledger/sql/queries", A, body={"name": "无参数", "sql": "SELECT 1 AS x", "params": []}); plain = r["data"]["id"]
s, r = call("POST", f"/ledger/sql/queries/{plain}/instances", A, body={"values": {}}); check("没有参数的查询不能生成实例", s == 400 and "没有参数" in msg((s, r)), (s, r))

# ── 4. 实例各自重算：订单追加新数据后 ──
more = [("A", date(2026, 9, 28), 1000), ("B", date(2026, 9, 28), 2000), ("A", date(2026, 8, 5), 99999)]
upload(A, "订单", csv(more), mode="append", ds=ORD)
rows2 = rows + more
s, r = call("POST", f"/ledger/sql/queries/{inst_a['id']}/refresh", A, body={}); check("实例 A 重算", s == 200 and r["data"]["rows"] == len(want("A", date(2026, 9, 1), rows2)), (s, r))
s, r = call("GET", "/ledger/datasets", A); ds = {d["name"]: d for d in r["data"]}
check("实例 B 没重算前不变；A 已更新（8 月那笔不在范围内）", ds["B店九月下旬"]["rowCount"] == len(wb_) and ds["分店订单（店铺=A，起=2026-09-01）"]["rowCount"] == len(want("A", date(2026, 9, 1), rows2)), {k: v["rowCount"] for k, v in ds.items()})
s, r = call("POST", f"/ledger/sql/queries/{inst_b['id']}/refresh", A, body={}); check("实例 B 重算", s == 200 and r["data"]["rows"] == len(want("B", date(2026, 9, 20), rows2)), (s, r))
s, r = call("GET", "/ledger/sql/queries", A); lastmsg = {q["name"]: q["lastStatus"] for q in r["data"]}
check("重算状态记录成功", lastmsg["B店九月下旬"] == "success")
# 修改实例的参数值（PUT 保存）会立即按新值重算
s, r = call("PUT", f"/ledger/sql/queries/{inst_a['id']}", A, body={"name": inst_a["name"], "sql": SQL, "params": DEFS, "values": {"店铺": "C", "起": "2026-08-01"}, "schedule": {"kind": "manual"}})
s2, r2 = call("GET", "/ledger/datasets", A); ds = {d["name"]: d for d in r2["data"]}
check("修改实例的值：数据集按新值重算", s == 200 and ds["分店订单（店铺=A，起=2026-09-01）"]["rowCount"] == len(want("C", date(2026, 8, 1), rows2)), (s, r))
s, r = call("PUT", f"/ledger/sql/queries/{inst_a['id']}", A, body={"name": inst_a["name"], "sql": SQL, "params": DEFS, "values": {"店铺": "C", "起": "坏"}, "schedule": {"kind": "manual"}})
check("改成不合法的值被拒，原来的数据保持", s == 400, (s, r))
s, r = call("GET", "/ledger/datasets", A); ds = {d["name"]: d for d in r["data"]}
check("被拒后数据集不变", ds["分店订单（店铺=A，起=2026-09-01）"]["rowCount"] == len(want("C", date(2026, 8, 1), rows2)))

finish()
