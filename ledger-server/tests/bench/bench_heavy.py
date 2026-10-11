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
        time.sleep(0.2)
threading.Thread(target=sampler, daemon=True).start()
def timed(label, fn):
    t = time.time(); out = fn(); ms = (time.time() - t) * 1000
    ok = out[0] if isinstance(out, tuple) else ""
    print(f"{ms:>10.0f} ms  {label}  -> {ok}  [RSS {rss():.0f} MB, 峰值 {peak[0]:.0f} MB]", flush=True)
    return out
s, r = call("GET", "/ledger/datasets", None); D = [d for d in r["data"] if d["name"] == "订单"][0]["id"]
spec = lambda **k: {"filters": [], "keyword": None, "sort": [], "rule": None, **k}
P = lambda body: call("POST", f"/ledger/datasets/{D}/pivot", None, body=body)
def cur(sp): return call("POST", f"/ledger/datasets/{D}/cursor", None, body=sp)
timed("无筛选游标（冷）", lambda: cur(spec()))
timed("按金额降序游标（全量排序）", lambda: cur(spec(sort=[{"column": "金额", "desc": True}])))
timed("按下单时间升序游标（全量排序）", lambda: cur(spec(sort=[{"column": "下单时间", "desc": False}])))
timed("店铺 × 月份：金额合计", lambda: P({"rows": [{"column": "店铺"}], "columns": [{"column": "下单时间", "granularity": "month"}], "values": [{"column": "金额", "agg": "sum"}]}))
timed("月度：金额合计 + 同比", lambda: P({"rows": [{"column": "下单时间", "granularity": "month"}], "values": [{"column": "金额", "agg": "sum"}, {"column": "金额", "agg": "sum", "show": "yoy"}]}))
timed("店铺：金额中位数 / P95", lambda: P({"rows": [{"column": "店铺"}], "values": [{"column": "金额", "agg": "median"}, {"column": "金额", "agg": "p95"}]}))
timed("店铺：订单号去重数（1 亿）", lambda: P({"rows": [{"column": "店铺"}], "values": [{"column": "订单号", "agg": "count_distinct"}]}))
timed("SQL：店铺内金额排名（窗口函数）", lambda: call("POST", "/ledger/sql/run", None, body={"sql": "SELECT * FROM (SELECT 店铺, 订单号, 金额, row_number() OVER (PARTITION BY 店铺 ORDER BY 金额 DESC) AS rn FROM 订单) WHERE rn <= 5 ORDER BY 店铺, rn"}))
print("峰值 RSS", round(peak[0]), "MB")
