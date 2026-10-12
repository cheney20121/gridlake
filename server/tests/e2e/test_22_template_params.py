"""模板「打开时填参数」：模板数据集来自参数化 SQL 查询时，使用模板只填参数值，按这组值生成（或复用）实例数据集

校验：发布时记下参数化来源（查询本身 / 实例 → 来源查询）与默认值；按参数生成数据集并改写工作簿引用，行数与独立计算一致；
同样的参数再用复用同一个数据集（并按最新数据重算，不生出重复的数据集）；缺值 / 类型错 / 既填参数又选数据集 / 非参数化数据集填参数的说明；
受限用户不能按参数生成（与「生成实例」一致，生成实例的接口也对受限用户关闭）；仍可以像以前一样选择数据集。
"""
from lib import *  # noqa: F403
for t in (A, Bo, R): call("GET", "/ledger/me", t)
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

ROWS = [("A", 10), ("A", 20), ("B", 100), ("B", 200), ("B", 300), ("C", 7)]
csv = lambda rs: "店铺,金额\n" + "\n".join(f"{s},{a}" for s, a in rs) + "\n"
ORD = upload(A, "订单", csv(ROWS))
SQL = "SELECT 店铺, 金额 FROM 订单 WHERE 店铺 = {{店铺}} AND 金额 >= {{下限}}"
DEFS = [{"name": "店铺", "kind": "text", "choices": []}, {"name": "下限", "kind": "number", "default": "0"}]
s, r = call("POST", "/ledger/sql/queries", A, body={"name": "分店订单", "sql": SQL, "params": DEFS, "values": {"店铺": "A"}, "saveAsDataset": "分店订单", "schedule": {"kind": "manual"}})
check("保存参数化查询并另存为数据集", s == 200 and r["data"]["datasetId"], (s, r))
Q, DQ = r["data"]["id"], r["data"]["datasetId"]

def workbook(ds_id, ds_name, name):
    """一个透视表页签：按店铺汇总金额（引用 ds_id 的 店铺、金额 两列）"""
    pivot = {"version": 1, "datasetId": ds_id, "datasetName": ds_name, "request": {"rows": [{"column": "店铺"}], "columns": [], "values": [{"column": "金额", "agg": "sum"}]}}
    wbj = gzip.compress(json.dumps({"name": name, "sheetOrder": ["s1"], "sheets": {"s1": {"id": "s1", "name": "透视表1", "cellData": {}, "custom": {"pivot": pivot}}}}).encode())
    s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote(name), A, raw=wbj)
    assert s == 200, (s, r)
    return r["data"]["id"]

def content(wb):
    s, raw = call("GET", f"/ledger/workbooks/{wb}/content", A, binary=True)
    assert s == 200, s
    try:
        return json.loads(gzip.decompress(raw))
    except OSError:
        return json.loads(raw)

def datasets():
    s, r = call("GET", "/ledger/datasets", A)
    return {d["id"]: d for d in r["data"]}

# ── 发布 ──
WB = workbook(DQ, "分店订单", "分店看板")
s, r = call("POST", "/ledger/templates", A, body={"workbookId": WB, "name": "分店看板模板"})
check("发布模板", s == 200, (s, r))
T = r["data"]
need = T["datasets"][0]
src = need.get("paramSource") or {}
check("模板数据集记下参数化来源：查询、参数定义、发布时的值", src.get("queryId") == Q and [p["name"] for p in src.get("params", [])] == ["店铺", "下限"] and src.get("values", {}).get("店铺") == "A", need)
KEY = need["key"]

def use(params=None, bindings=None, tok=A, name="B 店看板"):
    body = {"name": name, "bindings": bindings or {}, "clearSlicers": True}
    if params is not None:
        body["params"] = params
    return call("POST", f"/ledger/templates/{T['id']}/use", tok, body=body)

# ── 按参数生成 ──
before = set(datasets())
s, r = use({KEY: {"店铺": "B", "下限": "150"}})
check("填参数使用模板", s == 200, (s, r))
out = r["data"]
new_ids = set(datasets()) - before
check("生成了一个新数据集", len(new_ids) == 1, new_ids)
NEW = next(iter(new_ids)) if new_ids else None
ds = datasets().get(NEW, {})
check("新数据集的行数 = B 店金额 ≥ 150 的行数（200、300）", ds.get("rowCount") == 2, ds)
check("说明里写明按参数生成", any("按参数生成" in n for n in out.get("notes", [])), out)
p = content(out["workbook"]["id"])["sheets"]["s1"]["custom"]["pivot"]
check("工作簿的透视表改为引用新数据集", p["datasetId"] == NEW and p["datasetName"] == ds.get("name"), p)
s, r = call("POST", f"/ledger/datasets/{NEW}/pivot", A, body=p["request"])
cells = {x["keys"][0]: x["cells"][-1][0] for x in r["data"]["rows"] if x["kind"] == "data"} if s == 200 else {}
check("透视结果：只有 B 店，合计 500", cells == {"B": 500}, (s, cells))
s, r = call("GET", "/ledger/sql/queries", A)
inst = [q for q in r["data"] if q.get("datasetId") == NEW]
check("新数据集是参数化查询的实例（可在 SQL 工作台里各自重算、改参数）", len(inst) == 1 and inst[0]["templateId"] == Q and inst[0]["values"].get("店铺") == "B", inst)

# ── 同样的参数：复用（并按最新数据重算） ──
upload(A, "订单", csv([("B", 400)]), mode="append", ds=ORD)
before = set(datasets())
s, r = use({KEY: {"店铺": " B ", "下限": "150"}}, name="B 店看板 2")
check("同样的参数（值带空格也算相同）再次使用成功", s == 200, (s, r))
check("没有生成新的数据集", set(datasets()) == before, set(datasets()) - before)
p2 = content(r["data"]["workbook"]["id"])["sheets"]["s1"]["custom"]["pivot"] if s == 200 else {}
check("复用同一个数据集", p2.get("datasetId") == NEW, p2)
check("复用前按最新数据重算：B 店新追加的 400 也在（3 行）", datasets().get(NEW, {}).get("rowCount") == 3, datasets().get(NEW))
check("说明里写明使用已有的数据集", s == 200 and any("使用已有的数据集" in n for n in r["data"].get("notes", [])), r)

# 参数值与查询本身当前的值相同：直接用查询本身的数据集
s, r = use({KEY: {"店铺": "A"}}, name="A 店看板")
pa = content(r["data"]["workbook"]["id"])["sheets"]["s1"]["custom"]["pivot"] if s == 200 else {}
check("参数与查询本身的值相同（店铺 A、下限按默认 0）：用查询本身的数据集", s == 200 and pa.get("datasetId") == DQ, (s, pa, r))
s, r = use({KEY: {"店铺": "A", "下限": "0"}}, name="A 店看板 2")
pa = content(r["data"]["workbook"]["id"])["sheets"]["s1"]["custom"]["pivot"] if s == 200 else {}
check("显式填上默认值（下限 0）与不填算同一组参数", s == 200 and pa.get("datasetId") == DQ, (s, pa))

# ── 从实例发布：来源记为原查询 ──
WB2 = workbook(NEW, ds.get("name"), "实例看板")
s, r = call("POST", "/ledger/templates", A, body={"workbookId": WB2, "name": "实例模板"})
src2 = (r["data"]["datasets"][0].get("paramSource") or {}) if s == 200 else {}
check("从实例的数据集发布：来源是原来的参数化查询，默认值是实例的值", src2.get("queryId") == Q and src2.get("values", {}).get("店铺") == "B", (s, src2))

# ── 错误 ──
s, r = use({KEY: {"下限": "1"}}); check("缺值：说明缺哪个参数", s == 400 and "店铺" in msg((s, r)), (s, r))
s, r = use({KEY: {"店铺": "A", "下限": "很多"}}); check("类型不对：说明应为数字", s == 400 and "数字" in msg((s, r)), (s, r))
s, r = use({KEY: {"店铺": "A"}}, bindings={KEY: {"datasetId": ORD, "columns": {}}}); check("既填参数又选数据集", s == 400 and "不能既填参数又选择数据集" in msg((s, r)), (s, r))
s, r = use({"name:不存在": {"店铺": "A"}}); check("模板里没有的数据集", s == 400, (s, r))
WB3 = workbook(ORD, "订单", "普通看板")
s, r = call("POST", "/ledger/templates", A, body={"workbookId": WB3, "name": "普通模板"})
T3 = r["data"]
check("普通数据集发布的模板没有参数化来源", "paramSource" not in T3["datasets"][0], T3["datasets"][0])
s, r = call("POST", f"/ledger/templates/{T3['id']}/use", A, body={"name": "x", "bindings": {}, "params": {T3["datasets"][0]["key"]: {"店铺": "A"}}})
check("普通数据集不能填参数", s == 400 and "不是参数化查询" in msg((s, r)), (s, r))
s, r = use(None, bindings={KEY: {"datasetId": ORD, "columns": {}}}, name="选数据集")
check("仍可以像以前一样选择数据集", s == 200, (s, r))

# ── 受限用户 ──
s, r = call("GET", "/ledger/users?keyword=", R)
ids = {u["name"]: u["id"] for u in r["data"]}
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["A"]}); check("Bob 限定 A 店", s == 200, (s, r))
s, r = use({KEY: {"店铺": "C"}}, tok=Bo, name="Bob 看板")
check("受限用户不能按参数生成数据集（403，说明改为选择数据集）", s == 403 and "选择数据集" in msg((s, r)), (s, r))
s, r = call("POST", f"/ledger/sql/queries/{Q}/instances", Bo, body={"values": {"店铺": "C"}})
check("「生成实例」接口同样对受限用户关闭", s == 403, (s, r))
s, r = call("GET", "/ledger/sql/queries", A)
check("受限用户的尝试没有留下实例", not any(q.get("values", {}).get("店铺") == "C" for q in r["data"]), [q["name"] for q in r["data"]])
finish()
