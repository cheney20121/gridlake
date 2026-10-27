"""分类汇总：按某列分组，每组的行数与汇总；分组的先后与按这一列排序的游标完全一致（升序 / 降序、空值排最后），
所以前端能按行数在每组最后一行后面插入小计；带筛选 / 搜索 / 只看不合格；计算列也能分组；分组太多时拒绝；
数据范围受限的用户只汇总自己能看到的行。
"""
from lib import *  # noqa: F403

for t in (A, Bo, R):
    call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
def import_csv(name, csv, kinds):
    s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp(f"{name}.csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
    p = r["data"]
    s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": "create", "name": name, "columns": [{"source": c["source"], "title": c["title"], "kind": kinds.get(c["title"], "text")} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
    assert s == 200, (s, r)
    return r["data"]["datasetId"]

D = import_csv("分组订单", "单号,店铺,渠道,金额\n1,北京店,直播,10\n2,上海店,直播,20\n3,北京店,短视频,30\n4,,直播,40\n5,上海店,短视频,50\n6,广州店,直播,60\n7,北京店,直播,70\n", {"金额": "money", "单号": "integer"})
spec = lambda **k: {"filters": [], "keyword": None, "sort": [], "rule": None, **k}
SUM = [{"column": "金额", "func": "sum"}, {"column": "单号", "func": "count"}]

def groups(tok=A, **k):
    return call("POST", f"/ledger/datasets/{D}/groups", tok, body={**spec(**{x: y for x, y in k.items() if x in ("filters", "keyword", "violation")}), "column": k.get("column", "店铺"), "desc": k.get("desc", False), "items": k.get("items", SUM)})

def runs(tok=A, column="店铺", desc=False, **k):
    """按游标的顺序取出分组列，数连续的段：[(取值, 行数)]"""
    s, r = call("POST", f"/ledger/datasets/{D}/cursor", tok, body=spec(sort=[{"column": column, "desc": desc}], **k))
    c = r["data"]["cursorId"]
    s, r = call("GET", f"/ledger/datasets/{D}/rows?cursor={c}&offset=0&limit=100", tok)
    cols = [x["title"] for x in next(d for d in call("GET", "/ledger/datasets", tok)[1]["data"] if d["id"] == D)["columns"]]
    i = cols.index(column)
    out = []
    for row in r["data"]["rows"]:
        if out and out[-1][0] == row[i]:
            out[-1][1] += 1
        else:
            out.append([row[i], 1])
    return out

s, r = groups()
g = r["data"]["groups"]
check("按店铺分组：每组行数与汇总，空值排最后", s == 200 and [(x["key"], x["count"], x["values"]) for x in g] == [("上海店", 2, [70.0, 2]), ("北京店", 3, [110.0, 3]), ("广州店", 1, [60.0, 1]), (None, 1, [40.0, 1])], g)
check("分组先后与按店铺排序的游标一致", [[x["key"], x["count"]] for x in g] == runs(), (g, runs()))
s, r = groups(desc=True)
check("降序也一致", [[x["key"], x["count"]] for x in r["data"]["groups"]] == runs(desc=True), (r["data"]["groups"], runs(desc=True)))
s, r = groups(filters=[{"op": "in", "column": "渠道", "values": ["直播"]}])
check("带筛选：只算筛选出的行", [(x["key"], x["count"]) for x in r["data"]["groups"]] == [("上海店", 1), ("北京店", 2), ("广州店", 1), (None, 1)], r["data"]["groups"])
s, r = groups(keyword="北京")
check("带关键字搜索", [(x["key"], x["count"]) for x in r["data"]["groups"]] == [("北京店", 3)], r["data"]["groups"])
s, r = groups(violation={"column": "店铺", "kind": "required"})
check("只看不合格的行也能分组", [(x["key"], x["count"]) for x in r["data"]["groups"]] == [(None, 1)], r["data"]["groups"])
s, r = groups(column="金额", items=[])
check("数值列分组按数字排序", [x["key"] for x in r["data"]["groups"]] == [10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0], r["data"]["groups"])

call("POST", f"/ledger/datasets/{D}/calc", A, body={"name": "大单", "formula": '=IF([金额]>=50,"大单","小单")'})
s, r = groups(column="大单")
check("计算列也能分组", s == 200 and [(x["key"], x["count"]) for x in r["data"]["groups"]] == [("大单", 3), ("小单", 4)] and [[x["key"], x["count"]] for x in r["data"]["groups"]] == runs(column="大单"), r)
s, r = groups(items=[{"column": "店铺", "func": "sum"}])
check("文本列不能求和", s == 400, (s, r))
s, r = groups(column="没有这列")
check("列不存在", s >= 400, (s, r))

# ── 分组太多 ──
M = import_csv("很多单号", "单号,金额\n" + "".join(f"N{i},1\n" for i in range(10001)), {"金额": "money"})
s, r = call("POST", f"/ledger/datasets/{M}/groups", A, body={**spec(), "column": "单号", "items": []})
check("分组超过 1 万组：拒绝并说明", s == 400 and "超过 10000 种" in msg((s, r)), (s, msg((s, r))))

# ── 数据范围 ──
call("PUT", f"/ledger/data-scope/datasets/{D}", R, body={"scopeColumn": "店铺", "open": False})
call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["北京店"]})
s, r = groups(Bo)
check("受限用户只汇总自己能看到的行", s == 200 and [(x["key"], x["count"], x["values"][0]) for x in r["data"]["groups"]] == [("北京店", 3, 110.0)], (s, r))
call("DELETE", f"/ledger/data-scope/rules/{ids['Bob']}", R)
finish()
