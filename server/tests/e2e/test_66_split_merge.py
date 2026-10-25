"""拆分 / 合并列：拆分预览（每行能拆成几段、样例）；拆分 / 合并做成计算列（SPLITPART / TEXTBEFORE / TEXTAFTER / TEXTJOIN…），
不改存储的数据，之后追加进来的行自动拆好；能按拆出来的列筛选、透视；数据范围受限的用户预览只看到自己的行。
"""
from lib import *  # noqa: F403

for t in (A, Bo, R):
    call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
def import_csv(csv, mode, dataset_id=None):
    s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("收货.csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
    p = r["data"]
    s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": mode, "datasetId": dataset_id, "name": "收货", "columns": [{"source": c["source"], "title": c["title"], "kind": {"金额": "money"}.get(c["title"], "text")} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
    assert s == 200, (s, r)
    return r["data"]["datasetId"]

D = import_csv("单号,收货地址,金额\n1,广东-深圳-南山,10\n2,广东-广州-天河,20\n3,北京-北京-朝阳,30\n4,上海,40\n5,,50\n", "create")
spec = lambda **k: {"filters": [], "keyword": None, "sort": [], "rule": None, **k}

# ── 预览 ──
s, r = call("POST", f"/ledger/datasets/{D}/split/preview", A, body={"column": "收货地址", "separator": "-"})
p = r["data"]
check("预览：5 行里 4 行非空，最常见拆成 3 段，最多 3 段", s == 200 and (p["rows"], p["nonEmpty"], p["commonParts"], p["maxParts"]) == (5, 4, 3, 3), (s, r))
check("各段数的行数", [[c["parts"], c["rows"]] for c in p["counts"]] == [[1, 1], [3, 3]], p["counts"])
check("样例：原值和拆出来的各段，常见段数在前", p["samples"][0][1] and len(p["samples"][0][1]) == 3 and any(v == "上海" and parts == ["上海"] for v, parts in p["samples"]), p["samples"])
s, r = call("POST", f"/ledger/datasets/{D}/split/preview", A, body={"column": "收货地址", "separator": ""})
check("分隔符不能为空", s == 400, (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/split/preview", A, body={"column": "没有这列", "separator": "-"})
check("列不存在", s >= 400, (s, r))

# ── 拆分：每段一个计算列 ──
for i, name in enumerate(["省", "市", "区"], 1):
    s, r = call("POST", f"/ledger/datasets/{D}/calc", A, body={"name": name, "formula": f'=SPLITPART([收货地址],"-",{i})'})
    check(f"加计算列「{name}」", s == 200, (s, r))
cols = next(d for d in call("GET", "/ledger/datasets", A)[1]["data"] if d["id"] == D)["columns"]
check("同一秒里加的计算列按添加的先后排列（不按名字）", [c["title"] for c in cols if c.get("formula")] == ["省", "市", "区"], [c["title"] for c in cols])
s, r = call("POST", f"/ledger/datasets/{D}/distinct", A, body={**spec(), "column": "省"})
check("按拆出来的「省」取值", sorted(v["value"] for v in r["data"]["values"]) == ["", "上海", "北京", "广东"], r["data"]["values"])
s, r = call("POST", f"/ledger/datasets/{D}/cursor", A, body=spec(filters=[{"op": "in", "column": "市", "values": ["深圳", "广州"]}]))
check("按拆出来的「市」筛选", s == 200 and r["data"]["total"] == 2, (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/pivot", A, body={"rows": [{"column": "省"}], "values": [{"column": "金额", "agg": "sum"}]})
check("按拆出来的「省」透视", s == 200 and any(row["keys"] == ["广东"] and row["cells"][0][0] == 30 for row in r["data"]["rows"]), (s, r))

# ── 合并 ──
s, r = call("POST", f"/ledger/datasets/{D}/calc/preview", A, body={"formula": '=TEXTJOIN("/",TRUE,[省],[区],[单号])'})
check("合并预览：忽略空值，数字不带 .0", s == 200 and any("广东/南山/1" in json.dumps(x, ensure_ascii=False) for x in r["data"]["samples"]) and any("上海/4\"" in json.dumps(x, ensure_ascii=False) for x in r["data"]["samples"]), r)
s, r = call("POST", f"/ledger/datasets/{D}/calc", A, body={"name": "省区", "formula": '=TEXTJOIN("/",TRUE,[省],[区])'})
check("加合并列", s == 200, (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/calc", A, body={"name": "后半", "formula": '=TEXTAFTER([收货地址],"-")'})
s, r = call("POST", f"/ledger/datasets/{D}/distinct", A, body={**spec(), "column": "后半"})
check("TEXTAFTER：第一个分隔符之后", "深圳-南山" in [v["value"] for v in r["data"]["values"]], r["data"]["values"])

# ── 追加的行也自动拆好（计算列不改存储的数据）──
import_csv("单号,收货地址,金额\n6,浙江-杭州-西湖,60\n", "append", D)
s, r = call("POST", f"/ledger/datasets/{D}/cursor", A, body=spec(filters=[{"op": "in", "column": "市", "values": ["杭州"]}]))
check("新追加的行自动拆出「市」", s == 200 and r["data"]["total"] == 1, (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/calc", A, body={"name": "坏", "formula": '=SPLITPART([收货地址],"",1)'})
check("分隔符为空的公式拒绝", s == 400 and "分隔符" in msg((s, r)), (s, r))
s, r = call("POST", f"/ledger/datasets/{D}/calc", A, body={"name": "坏", "formula": '=TEXTJOIN("-",[省],[市])'})
check("TEXTJOIN 第二个参数不对：说明怎么写", s == 400 and "TRUE" in msg((s, r)), (s, r))

# ── 数据范围 ──
call("PUT", f"/ledger/data-scope/datasets/{D}", R, body={"scopeColumn": "单号", "open": False})
call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["4", "5"]})
s, r = call("POST", f"/ledger/datasets/{D}/split/preview", Bo, body={"column": "收货地址", "separator": "-"})
check("受限用户的拆分预览只看到自己的行", s == 200 and r["data"]["rows"] == 2 and r["data"]["nonEmpty"] == 1, (s, r))
call("DELETE", f"/ledger/data-scope/rules/{ids['Bob']}", R)
finish()
