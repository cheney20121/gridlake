"""排序 / 筛选游标的建表耗时、进程内存峰值与取行延迟。每个场景请先重启服务，峰值才互不影响：

    python3 tests/bench/bench_cursor.py <ledger-server 进程号> <场景>

场景：sort_money（金额降序）、sort_time（下单时间升序）、filter（店铺筛选，不排序）、filter_sort（筛选 + 排序）
"""
import os, sys, time, subprocess, threading
os.environ["LEDGER_E2E_BASE"] = "http://127.0.0.1:18950/api"
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "e2e"))
from lib import call

PID, CASE = int(sys.argv[1]), sys.argv[2]
rss = lambda: int(subprocess.check_output(["ps", "-o", "rss=", "-p", str(PID)]).strip()) / 1024
peak = [0]


def sampler():
    while True:
        try:
            peak[0] = max(peak[0], rss())
        except Exception:
            return
        time.sleep(0.1)


threading.Thread(target=sampler, daemon=True).start()
s, r = call("GET", "/ledger/datasets", None)
D = [d for d in r["data"] if d["name"] == "订单"][0]["id"]
base = {"filters": [], "keyword": None, "sort": [], "rule": None}
cases = {
    "sort_money": {**base, "sort": [{"column": "金额", "desc": True}]},
    "sort_time": {**base, "sort": [{"column": "下单时间", "desc": False}]},
    "filter": {**base, "filters": [{"op": "between", "column": "金额", "min": 100, "max": 500}]},
    "filter_sort": {**base, "filters": [{"op": "between", "column": "金额", "min": 100, "max": 500}], "sort": [{"column": "金额", "desc": True}]},
}
spec = cases[CASE]
print(f"基线 RSS {rss():.0f} MB")
t = time.time()
s, r = call("POST", f"/ledger/datasets/{D}/cursor", None, body=spec)
ms = (time.time() - t) * 1000
assert s == 200, r
cur = r["data"]
print(f"{CASE}: 建游标 {ms:.0f} ms，总行数 {cur['total']:,}，直通 {cur['direct']}  [RSS {rss():.0f} MB, 峰值 {peak[0]:.0f} MB]", flush=True)
time.sleep(1)
print(f"  空闲后 RSS {rss():.0f} MB")
# 取行：开头 / 中间 / 末尾，各取 50 行
for off in (0, cur["total"] // 2, max(cur["total"] - 50, 0)):
    t = time.time()
    s, r = call("GET", f"/ledger/datasets/{D}/rows?cursor={cur['cursorId']}&offset={off}&limit=50")
    w = r["data"]
    print(f"  取行 offset={off:>11,}: {(time.time() - t) * 1000:6.0f} ms，{len(w['rows'])} 行，expired={w['expired']}，首行 {w['rows'][0][:3] if w['rows'] else None}")
# 前几行校验：排序是否正确
s, r = call("GET", f"/ledger/datasets/{D}/rows?cursor={cur['cursorId']}&offset=0&limit=3")
print("  前三行", r["data"]["rows"])
print(f"峰值 RSS {peak[0]:.0f} MB")
