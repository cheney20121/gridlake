"""gridlake open：一条命令在本机打开数据文件

用测试自己启动的 `gridlake open`（另一个端口、临时库、不开浏览器）：Parquet 直接查询、CSV 导入；
同一批文件再打开一次不会重复建数据集（外部数据集刷新、导入的整体替换）；不需要登录、只听本机；找不到文件时直接报错退出。
"""
import json
import os
import shutil
import subprocess
import urllib.request

from lib import *  # noqa: F403

BIN = os.environ["LEDGER_E2E_BIN"]
TMP = os.path.join(os.environ["LEDGER_E2E_TMP"], "open")
os.makedirs(TMP, exist_ok=True)
FIX = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures", "external")
PQ = os.path.join(TMP, "orders.parquet")
shutil.copy(os.path.join(FIX, "orders-2026-09.parquet"), PQ)
CSV = os.path.join(TMP, "客户.csv")
with open(CSV, "w", encoding="utf-8") as f:
    f.write("客户,数量\n甲,3\n乙,4\n")
PORT = 18799
DB = os.path.join(TMP, "open.duckdb")
API = f"http://127.0.0.1:{PORT}/api"


def start(*paths):
    env = {k: v for k, v in os.environ.items() if not k.startswith("LEDGER_")}
    p = subprocess.Popen([BIN, "open", *paths, "--port", str(PORT), "--db", DB, "--no-browser"], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env)
    lines = []
    for line in p.stdout:
        lines.append(line.rstrip())
        if "Ctrl+C" in line:
            break
    return p, lines


def stop(p):
    p.terminate()
    p.wait(timeout=20)


def datasets():
    with urllib.request.urlopen(f"{API}/ledger/datasets") as r:
        return {d["name"]: d for d in json.load(r)["data"]}


p, out = start(PQ, CSV)
check("启动并打印打开了什么", any("orders：直接查询 1 个 Parquet 文件，1000 行" in x for x in out) and any("客户：导入 2 行" in x for x in out), out)
url = next((x for x in out if "/excel?ds=" in x or "接口地址" in x), "")
check("给出地址（只听本机）", "http://127.0.0.1:18799" in url, out)
check("没有刷屏的日志（默认只显示警告）", not any(" INFO " in x for x in out), out)
ds = datasets()
check("不需要登录就能访问；两个数据集", set(ds) == {"orders", "客户"} and ds["orders"]["kind"] == "external" and ds["客户"]["rowCount"] == 2, list(ds))
first = {k: v["id"] for k, v in ds.items()}
stop(p)

with open(CSV, "a", encoding="utf-8") as f:
    f.write("丙,5\n")
p, out = start(PQ, CSV)
ds = datasets()
check("再打开一次：还是这两个数据集（id 不变），CSV 的数据整体替换", {k: v["id"] for k, v in ds.items()} == first and ds["客户"]["rowCount"] == 3, (out, {k: (v["id"], v["rowCount"]) for k, v in ds.items()}))
stop(p)

r = subprocess.run([BIN, "open", os.path.join(TMP, "没有这个文件.csv"), "--port", str(PORT), "--db", DB, "--no-browser"], capture_output=True, text=True, timeout=60)
check("找不到文件：报错退出", r.returncode != 0 and "找不到" in (r.stdout + r.stderr), (r.returncode, r.stdout, r.stderr))
r = subprocess.run([BIN, "open", "--port"], capture_output=True, text=True, timeout=60)
check("参数不对：给出用法", r.returncode != 0 and "gridlake open" in (r.stdout + r.stderr), (r.returncode, r.stderr))
finish()
