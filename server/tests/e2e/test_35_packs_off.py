# env: LEDGER_PACKS=none
"""场景包：没启用电商场景包时，台账相关的接口、内置数据集、内置模板都不出现，核心功能照常"""
from lib import *  # noqa: F403

s, r = call("GET", "/ledger/me", A)
check("/me 报告电商场景包未启用", s == 200 and r["data"]["packs"] == {"ecommerce": False}, (s, r))

for method, path in [("GET", "/ledger/shops"), ("GET", "/ledger/periods"), ("GET", "/ledger/close?period=2026-09"), ("GET", "/ledger/coverage"),
                     ("POST", "/ledger/details/rows"), ("GET", "/ledger/balances?period=2026-09"), ("GET", "/ledger/withdrawals?period=2026-09"),
                     ("GET", "/ledger/analysis/breakdown?period=2026-09&by=channel"), ("GET", "/ledger/imports"), ("GET", "/ledger/bank/imports")]:
    s, r = call(method, path, A, body={} if method == "POST" else None)
    check(f"{method} {path.split('?')[0]}：404（接口不存在）", s == 404 and "接口不存在" in str(r.get("msg", "")), (s, r))

s, r = call("GET", "/ledger/datasets", A)
names = [d["name"] for d in r["data"]] if s == 200 else []
check("数据集列表里没有内置的「资金流水」", s == 200 and "资金流水" not in names, names)

s, r = call("GET", "/ledger/templates", A)
tpl = {t["id"] for t in r["data"]} if s == 200 else set()
check("内置模板：电商专用的不出现", s == 200 and not ({"builtin_refund_analysis", "builtin_qianchuan_roi", "builtin_platform_pnl"} & tpl), tpl)
check("内置模板：通用的还在", {"builtin_ar_ap", "builtin_bank_rec", "builtin_stock", "builtin_monthly_ops"} <= tpl, tpl)

# 核心功能不受影响：导入通用数据集、SQL
boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("销售.csv", "地区,金额\n华东,10\n华东,5\n华北,7\n"), ctype=f"multipart/form-data; boundary={boundary}")
check("通用数据集导入预览可用", s == 200, (s, str(r)[:200]))
p = r["data"]
s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": "create", "name": "销售", "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
check("提交导入", s == 200, (s, str(r)[:200]))
s, r = call("POST", "/ledger/sql/run", A, body={"sql": "SELECT 地区, sum(金额) AS 合计 FROM 销售 GROUP BY 1 ORDER BY 1"})
check("SQL 工作台可用", s == 200 and {x[0]: x[1] for x in r["data"]["rows"]} == {"华北": 7.0, "华东": 15.0}, (s, str(r)[:200]))

finish()
