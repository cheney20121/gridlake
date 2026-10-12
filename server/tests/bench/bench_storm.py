"""并发风暴：同时发 4 个「分位数透视」+ 2 个「按金额 / 时间排序的游标」，看总耗时、各请求结果与进程内存峰值
用法：[APPROX=1] LEDGER_E2E_TIMEOUT=900 python3 bench_storm.py <服务进程 PID> [pivot|cursor]（APPROX=1 时分位数用近似版）（只发其中一类，用来分别看各自的内存峰值）"""
import os, sys, time, json, subprocess, threading
os.environ["LEDGER_E2E_BASE"] = "http://127.0.0.1:18950/api"
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "e2e"))
from lib import call
PID = int(sys.argv[1])
def rss(): return int(subprocess.check_output(["ps", "-o", "rss=", "-p", str(PID)]).strip()) / 1024
peak = [0]
def sampler():
    while True:
        try: peak[0] = max(peak[0], rss())
        except Exception: return
        time.sleep(0.1)
threading.Thread(target=sampler, daemon=True).start()
s, r = call("GET", "/ledger/datasets", None); D = [d for d in r["data"] if d["name"] == "订单"][0]["id"]
spec = lambda **k: {"filters": [], "keyword": None, "sort": [], "rule": None, **k}
jobs = [("分位数透视", lambda: call("POST", f"/ledger/datasets/{D}/pivot", None, body={"rows": [{"column": "店铺"}], "values": [{"column": "金额", "agg": "approx_median" if os.environ.get("APPROX") else "median"}, {"column": "金额", "agg": "approx_p95" if os.environ.get("APPROX") else "p95"}], "filters": [{"op": "in", "column": "渠道", "values": [c]}]})) for c in ["直播", "商品卡", "短视频", "自然"]]
jobs += [("金额排序游标", lambda: call("POST", f"/ledger/datasets/{D}/cursor", None, body=spec(sort=[{"column": "金额", "desc": True}]))), ("时间排序游标", lambda: call("POST", f"/ledger/datasets/{D}/cursor", None, body=spec(sort=[{"column": "下单时间", "desc": False}])))]
only = sys.argv[2] if len(sys.argv) > 2 else ""
jobs = [j for j in jobs if only == "" or (only == "pivot") == j[0].startswith("分位数")]
out = []
def run(name, fn):
    t = time.time(); s, r = fn(); out.append((name, s, time.time() - t, (r.get("msg") if isinstance(r, dict) else "")[:60]))
t0 = time.time(); ths = [threading.Thread(target=run, args=j) for j in jobs]; [t.start() for t in ths]; [t.join() for t in ths]
for name, s, dt, m in sorted(out, key=lambda x: x[2]): print(f"  {name:<10} HTTP {s}  {dt:6.1f}s  {m}")
print(f"总耗时 {time.time() - t0:.1f}s  成功 {sum(1 for o in out if o[1] == 200)}/{len(out)}  峰值 RSS {peak[0]:.0f} MB")
