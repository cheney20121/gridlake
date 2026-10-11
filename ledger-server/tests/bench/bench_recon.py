"""逐笔对账在大数据量下的耗时：造 N 行银行流水与 N 行企业账（约 90% 能一一对上），测创建、自动勾对、汇总、翻页、公式下推。

    LEDGER_ADDR=127.0.0.1:18951 LEDGER_DB=/tmp/recon-bench/ledger.duckdb target/release/ledger-server &
    python3 tests/bench/bench_recon.py <行数> [keys|nokeys]

keys：两侧有参考号（常见：订单号 / 流水号）；nokeys：只有金额 + 日期，大量相同金额的行（最坏情况）
"""
import os, random, sys, time, subprocess
os.environ["LEDGER_E2E_BASE"] = "http://127.0.0.1:18951/api"
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "e2e"))
from lib import call

N, MODE = int(sys.argv[1]), (sys.argv[2] if len(sys.argv) > 2 else "keys")
boundary = "XB"
def mp(name, path):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + open(path, "rb").read() + f"\r\n--{boundary}--\r\n".encode()
def import_ds(name, path):
    t = time.time()
    s, r = call("POST", "/ledger/datasets/import/preview", None, raw=mp(name + ".csv", path), ctype=f"multipart/form-data; boundary={boundary}")
    p = r["data"]
    s, r = call("POST", "/ledger/datasets/import/commit", None, body={"token": p["token"], "mode": "create", "name": name, "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
    assert s == 200, (s, r)
    print(f"  导入 {name} {N} 行：{time.time() - t:.1f}s")

random.seed(1)
os.makedirs("/tmp/recon-bench", exist_ok=True)
with open("/tmp/recon-bench/bank.csv", "w") as fb, open("/tmp/recon-bench/book.csv", "w") as fk:
    fb.write("交易日期,收支金额,流水号,摘要\n"); fk.write("凭证日期,金额,参考号,摘要\n")
    for i in range(N):
        d = random.randint(0, 364)
        c = random.randint(1, 20000 if MODE == "nokeys" else 5000000) / 100
        day = f"2026-{1 + d // 31:02d}-{1 + d % 28:02d}"
        ref = f"R{i:09d}" if MODE == "keys" else ""
        if random.random() < 0.9:
            fb.write(f"{day},{c},{ref},b{i}\n"); fk.write(f"{day},{c},{ref},k{i}\n")
        else:
            fb.write(f"{day},{c},{ref if MODE == 'keys' else ''},b{i}\n")
            fk.write(f"{day},{c + 0.77},{('X' + ref) if MODE == 'keys' else ''},k{i}\n")
import_ds("银行流水", "/tmp/recon-bench/bank.csv")
import_ds("企业账", "/tmp/recon-bench/book.csv")

keys = MODE == "keys"
d = {"left": {"dataset": "银行流水", "label": "银行", "amount": "收支金额", "date": "交易日期", "keys": ["流水号"] if keys else [], "memo": "摘要"},
     "right": {"dataset": "企业账", "label": "账面", "amount": "金额", "date": "凭证日期", "keys": ["参考号"] if keys else [], "memo": "摘要"},
     "tolerance": 0, "days": 3, "groupByKey": False}
def timed(label, *a, **k):
    t = time.time(); s, r = call(*a, **k); ms = (time.time() - t) * 1000
    print(f"  {label}：{ms:.0f} ms" + ("" if s == 200 else f"  !! {s} {r}")); return r
timed("预览", "POST", "/ledger/recons/preview", None, body={"def": d})
r = timed("创建", "POST", "/ledger/recons", None, body={"name": "银行对账", "def": d}); RC = r["data"]
r = timed("试算", "POST", f"/ledger/recons/{RC}/auto", None, body={"dryRun": True}); print("   ", {k: v for k, v in r["data"].items() if k != "warnings"}, r["data"]["warnings"])
t = time.time(); r = timed("自动勾对", "POST", f"/ledger/recons/{RC}/auto", None, body={}); print("   ", {k: v for k, v in r["data"].items() if k != "warnings"})
timed("汇总（截至 6 月底）", "GET", f"/ledger/recons/{RC}/summary?asOf=2026-06-30", None)
timed("未勾对列表第一页", "GET", f"/ledger/recons/{RC}/items?side=left&status=open&limit=100", None)
timed("按金额找勾对对象", "GET", f"/ledger/recons/{RC}/items?side=right&status=open&amount=12.34", None)
timed("勾对组第一页", "GET", f"/ledger/recons/{RC}/groups?limit=50", None)
timed("DSUMIFS 条件求和", "POST", f"/ledger/datasets/{RC}/aggregate", None, body={"calls": [{"func": "sum", "column": "金额", "criteria": [{"column": "来源", "value": "银行"}, {"column": "状态", "value": "未勾对"}]}]})
timed("透视 来源×状态", "POST", f"/ledger/datasets/{RC}/pivot", None, body={"rows": [{"column": "来源"}], "columns": [{"column": "状态"}], "values": [{"column": "金额", "agg": "sum"}]})
timed("结果数据集游标（按日期排序）", "POST", f"/ledger/datasets/{RC}/cursor", None, body={"filters": [], "keyword": None, "sort": [{"column": "日期", "desc": False}], "rule": None})
