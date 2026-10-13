"""数据范围的「维度」：同一个用户在不同数据集上受不同规则限制（店铺 / 地区 …），旧写法（mode=shops、shops 字段）仍可用"""
from lib import *  # noqa: F403
for t in (A, Bo, R): call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}

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

orders = import_ds(A, "订单", "店铺,金额\nA,10\nA,20\nB,100\nC,1000\n")
sales = import_ds(A, "区域销售", "地区,金额\n华东,1\n华北,2\n华南,3\n华东,4\n")
costs = import_ds(A, "成本", "项目,金额\n房租,5\n工资,6\n")

# 绑定：订单按「店铺」维度的店铺列；区域销售按「地区」维度的地区列；成本所有人可见
s, r = call("PUT", f"/ledger/data-scope/datasets/{orders}", R, body={"scopeColumn": "店铺", "open": False}); check("订单按店铺列（维度默认「店铺」）", s == 200, (s, r))
s, r = call("PUT", f"/ledger/data-scope/datasets/{sales}", R, body={"scopeColumn": "地区", "scopeDimension": "地区", "open": False}); check("区域销售按「地区」维度的地区列", s == 200, (s, r))
s, r = call("PUT", f"/ledger/data-scope/datasets/{costs}", R, body={"open": True}); check("成本所有人可见", s == 200, (s, r))
s, r = call("PUT", f"/ledger/data-scope/datasets/{sales}", R, body={"scopeColumn": "地区", "scopeDimension": "x" * 30}); check("维度名称太长被拒绝", s == 400, (s, r))

# 只限制「地区」
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "dimension": "地区", "mode": "values", "values": ["华东", "华南"]}); check("给 Bob 设置地区规则", s == 200, (s, r))
s, r = call("GET", "/ledger/me", Bo)
sc = r["data"]["scope"]
check("me：受限，dimensions 里只有地区；shops（店铺维度）为空", sc["restricted"] and sc["dimensions"] == {"地区": ["华东", "华南"]} and sc["shops"] == [], sc)
s, r = call("GET", "/ledger/datasets", Bo); names = {d["name"]: d["rowCount"] for d in r["data"]}
check("订单不受限（店铺维度没有规则）、区域销售只剩华东 / 华南、成本可见", names.get("订单") == 4 and names.get("区域销售") == 3 and names.get("成本") == 2, names)

# 再限制「店铺」：两个维度同时生效
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "dimension": "店铺", "mode": "values", "values": ["A"]}); check("再给 Bob 设置店铺规则", s == 200, (s, r))
s, r = call("GET", "/ledger/me", Bo); sc = r["data"]["scope"]
check("me：两个维度，shops 是店铺维度的取值", sc["dimensions"] == {"地区": ["华东", "华南"], "店铺": ["A"]} and sc["shops"] == ["A"], sc)
s, r = call("GET", "/ledger/datasets", Bo); names = {d["name"]: d["rowCount"] for d in r["data"]}
check("订单只剩店铺 A，区域销售仍是华东 / 华南", names.get("订单") == 2 and names.get("区域销售") == 3, names)
spec = {"filters": [], "keyword": None, "sort": [], "rule": None}
s, r = call("POST", f"/ledger/datasets/{orders}/cursor", Bo, body=spec); cb = r["data"]
s, r = call("POST", f"/ledger/datasets/{orders}/cursor", A, body=spec); ca = r["data"]
check("游标按范围区分：Bob 2 行、Alice 4 行", cb["total"] == 2 and ca["total"] == 4, (cb, ca))
s, r = call("POST", "/ledger/sql/run", Bo, body={"sql": "SELECT 地区, sum(金额) AS 合计 FROM 区域销售 GROUP BY 1 ORDER BY 1"})
check("SQL 也按地区受限", s == 200 and {x[0]: x[1] for x in r["data"]["rows"]} == {"华东": 5.0, "华南": 3.0}, (s, str(r)[:200]))

# 概览
s, r = call("GET", "/ledger/data-scope", R); ov = r["data"]
check("概览：维度列表含店铺和地区", {"店铺", "地区"} <= set(ov["dimensions"]), ov["dimensions"])
b = {d["name"]: d for d in ov["datasets"]}
check("概览：数据集的绑定带维度", b["区域销售"]["scopeDimension"] == "地区" and b["区域销售"]["effectiveDimension"] == "地区" and b["订单"]["effectiveDimension"] == "店铺" and b["成本"]["effective"] == "open", b)
rules = {(x["subjectName"], x["dimension"]): x for x in ov["rules"]}
check("概览：规则按（用户，维度）列出，取值字段叫 values", rules[("Bob", "地区")]["values"] == ["华东", "华南"] and rules[("Bob", "店铺")]["mode"] == "values", list(rules))

# 默认规则按维度生效；个人「不限」覆盖默认规则
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": "*", "dimension": "地区", "mode": "values", "values": ["华北"]}); check("默认规则：地区只能看华北", s == 200, (s, r))
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "dimension": "地区", "mode": "all"}); check("Bob 的地区改为不限", s == 200, (s, r))
s, r = call("GET", "/ledger/datasets", Bo); names = {d["name"]: d["rowCount"] for d in r["data"]}
check("Bob 的个人规则优先于默认规则：区域销售全可见（4 行）", names.get("区域销售") == 4, names)

# 只删某个维度的规则
s, r = call("DELETE", f"/ledger/data-scope/rules/{ids['Bob']}?dimension=%E5%BA%97%E9%93%BA", R); check("只删店铺规则", s == 200, (s, r))
s, r = call("GET", "/ledger/me", Bo); sc = r["data"]["scope"]
check("店铺规则删了，只剩地区的默认规则被个人「不限」覆盖：不受限", not sc["restricted"], sc)
s, r = call("DELETE", f"/ledger/data-scope/rules/{ids['Bob']}", R); check("删掉 Bob 的全部规则", s == 200, (s, r))
s, r = call("GET", "/ledger/me", Bo); sc = r["data"]["scope"]
check("回到默认规则：地区只能看华北", sc["dimensions"] == {"地区": ["华北"]}, sc)

# 旧写法
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["B"]}); check("旧写法 mode=shops + shops 字段仍可用（维度默认店铺）", s == 200, (s, r))
s, r = call("GET", "/ledger/me", Bo); sc = r["data"]["scope"]
check("旧写法生效：店铺 B", sc["dimensions"].get("店铺") == ["B"] and sc["shops"] == ["B"], sc)
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "x"}); check("mode 非法被拒绝", s == 400, (s, r))

# 英文错误信息
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "x"}, headers={"x-lang": "en-US"})
check("错误信息有英文", "values (specific values)" in r.get("msg", ""), r)
finish()
