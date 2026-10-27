"""gridlake demo：一条命令生成示例数据和三个示例工作簿

用测试自己启动的 `gridlake demo`（另一个端口、临时库、不开浏览器、2 万行订单）：三个数据集、三个工作簿，
工作簿里的模板引用都换成了示例数据集；再运行一次直接打开、不重复生成；--reset 重新生成；参数不对直接报错退出。
"""
import json
import os
import subprocess
import urllib.parse
import urllib.request

from lib import *  # noqa: F403

BIN = os.environ["LEDGER_E2E_BIN"]
TMP = os.path.join(os.environ["LEDGER_E2E_TMP"], "demo")
os.makedirs(TMP, exist_ok=True)
PORT = 18798
DB = os.path.join(TMP, "demo.duckdb")
API = f"http://127.0.0.1:{PORT}/api"
# 前端页面：CI 里没有构建前端（web/dist），给一个只有 index.html 的目录，结果不依赖本机有没有构建过
WEB = os.path.join(TMP, "web")
os.makedirs(WEB, exist_ok=True)
with open(os.path.join(WEB, "index.html"), "w") as f:
    f.write("<!doctype html><title>Gridlake</title>")


def start(*extra):
    env = {k: v for k, v in os.environ.items() if not k.startswith("LEDGER_")}
    env["LEDGER_WEB_DIR"] = WEB
    p = subprocess.Popen([BIN, "demo", "--rows", "20000", "--port", str(PORT), "--db", DB, "--no-browser", *extra], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env)
    lines = []
    for line in p.stdout:
        lines.append(line.rstrip())
        if "Ctrl+C" in line:
            break
    return p, lines


def stop(p):
    p.terminate()
    p.wait(timeout=20)


def get(path):
    with urllib.request.urlopen(API + path) as r:
        return json.load(r)["data"]


def post(path, body):
    req = urllib.request.Request(API + path, data=json.dumps(body).encode(), method="POST", headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req) as r:
        return json.load(r)["data"]


p, out = start()
check("打印生成了什么", any("订单：导入 20000 行" in x and "示例：月度经营分析" in x for x in out) and any("库存流水" in x for x in out) and any("往来明细" in x for x in out), out)
check("给出工作簿地址（只听本机）", any(f"http://127.0.0.1:{PORT}/excel?wb=" in x for x in out), out)
check("没有刷屏的日志、没有模板替换警告", not any(" INFO " in x or " WARN " in x for x in out), out)
ds = {d["name"]: d for d in get("/ledger/datasets")}
check("三个数据集", {"订单", "库存流水", "往来明细"} <= set(ds) and ds["订单"]["rowCount"] == 20000, list(ds))
kinds = {c["title"]: c["kind"] for c in ds["订单"]["columns"]}
check("订单的列类型（编号、整数、金额、日期时间、是否）", kinds == {"订单号": "id", "店铺": "text", "下单时间": "datetime", "商品": "text", "渠道": "text", "状态": "text", "数量": "integer", "订单金额": "money", "已退款": "boolean"}, kinds)
wbs = {w["name"]: w for w in get("/ledger/workbooks") if w["name"].startswith("示例：")}
check("三个示例工作簿", set(wbs) == {"示例：月度经营分析", "示例：库存进销存", "示例：应收应付对账"}, list(wbs))
url = next(x for x in out if "/excel?wb=" in x)
check("打开的是月度经营分析", wbs["示例：月度经营分析"]["id"] in url, url)
req = urllib.request.Request(f"{API}/ledger/workbooks/{wbs['示例：月度经营分析']['id']}/content")
with urllib.request.urlopen(req) as r:
    raw = r.read()
try:
    import gzip
    text = gzip.decompress(raw).decode()
except OSError:
    text = raw.decode()
check("模板引用换成了示例数据集", ds["订单"]["id"] in text and "$orders" not in text, text[:200])
v = post(f"/ledger/datasets/{ds['订单']['id']}/aggregate", {"calls": [{"func": "max", "column": "下单时间", "criteria": []}, {"func": "count", "criteria": [{"column": "已退款", "value": "true"}]}]})["values"]
check("订单截止到今天附近，有已退款的", str(v[0])[:7] >= time.strftime("%Y-%m", time.localtime(time.time() - 40 * 86400)) and v[1] > 0, v)
types = sorted(x["value"] for x in post(f"/ledger/datasets/{ds['往来明细']['id']}/distinct", {"column": "类型"})["values"])
check("往来明细的类型正好是模板要求的四种", types == ["付款", "应付", "应收", "收款"], types)
stop(p)

p, out = start()
check("再运行一次：不重复生成", any("已经生成过" in x for x in out) and len([w for w in get("/ledger/workbooks") if w["name"].startswith("示例：")]) == 3 and len(get("/ledger/datasets")) == len(ds), out)
stop(p)

p, out = start("--reset")
new = {d["name"]: d["id"] for d in get("/ledger/datasets")}
check("--reset：重新生成（新的数据集）", any("订单：导入 20000 行" in x for x in out) and new["订单"] != ds["订单"]["id"] and len([w for w in get("/ledger/workbooks") if w["name"].startswith("示例：")]) == 3, out)
stop(p)

r = subprocess.run([BIN, "demo", "--rows", "10", "--db", DB], capture_output=True, text=True, env={k: v for k, v in os.environ.items() if not k.startswith("LEDGER_")}, timeout=30)
check("行数不对：直接报错退出", r.returncode != 0 and "1000" in (r.stdout + r.stderr), (r.returncode, r.stdout, r.stderr))
finish()
