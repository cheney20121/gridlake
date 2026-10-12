"""长时间混合负载（浸泡测试）：几个线程持续发透视、公式汇总、SQL、游标、列值请求，记录各类请求的延迟与错误，
并采样服务进程内存，用来发现内存泄漏、性能随时间劣化、偶发 5xx。

用法：python3 soak.py <服务进程 PID> [分钟数=10] [并发=4] [结果文件=soak-result.json]
前置：服务在 127.0.0.1:18950（LEDGER_E2E_BASE 可改），已有名为「订单」的数据集（列：订单号 店铺 下单时间 商品 渠道 状态 数量 金额，
见 gen_base.py / import_base.py）。没有登录（LEDGER_AUTH_MODE=off，本机监听）。
通过条件（退出码 0）：服务一直存活；没有 5xx（503「服务繁忙」排队超时不算错误，但会计数）；
预热后内存没有持续增长（结束时比预热结束时高出 60% + 500 MB 以上视为泄漏）。
"""
import json
import os
import random
import statistics
import subprocess
import sys
import threading
import time

os.environ.setdefault("LEDGER_E2E_BASE", "http://127.0.0.1:18950/api")
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "e2e"))
from lib import call  # noqa: E402

PID = int(sys.argv[1])
MINUTES = float(sys.argv[2]) if len(sys.argv) > 2 else 10
WORKERS = int(sys.argv[3]) if len(sys.argv) > 3 else 4
OUT = sys.argv[4] if len(sys.argv) > 4 else "soak-result.json"
WARMUP_FRACTION = 0.2


def rss_mb():
    try:
        return int(subprocess.check_output(["ps", "-o", "rss=", "-p", str(PID)]).strip()) / 1024
    except Exception:
        return None


s, r = call("GET", "/ledger/datasets", None)
D = next(d["id"] for d in r["data"] if d["name"] == "订单")
SHOPS = [f"店铺{i:03d}" for i in range(300)]
CHANNELS = ["直播", "商品卡", "短视频", "自然", "达人"]
spec = lambda **k: {"filters": [], "keyword": None, "sort": [], "rule": None, **k}  # noqa: E731


def op_cursor_rows():
    shop = random.choice(SHOPS)
    s, r = call("POST", f"/ledger/datasets/{D}/cursor", None, body=spec(filters=[{"op": "in", "column": "店铺", "values": [shop]}]))
    if s == 200 and r["data"]["total"] > 0:
        c = r["data"]["cursorId"]
        return call("GET", f"/ledger/datasets/{D}/rows?cursor={c}&offset=0&limit=100", None)[0]
    return s


def op_pivot_month():
    return call("POST", f"/ledger/datasets/{D}/pivot", None, body={"rows": [{"column": "店铺"}], "columns": [{"column": "下单时间", "granularity": "month"}], "values": [{"column": "金额", "agg": "sum"}], "filters": [{"op": "in", "column": "渠道", "values": [random.choice(CHANNELS)]}]})[0]


def op_pivot_small():
    return call("POST", f"/ledger/datasets/{D}/pivot", None, body={"rows": [{"column": "渠道"}], "columns": [{"column": "状态"}], "values": [{"agg": "count"}, {"column": "金额", "agg": "sum"}]})[0]


def op_dsumifs():
    calls = [{"func": "sum", "column": "金额", "criteria": [{"column": "店铺", "value": random.choice(SHOPS)}, {"column": "状态", "value": "已完成"}]} for _ in range(20)]
    return call("POST", f"/ledger/datasets/{D}/aggregate", None, body={"calls": calls})[0]


def op_sql():
    return call("POST", "/ledger/sql/run", None, body={"sql": "SELECT 渠道, 状态, count(*) AS 笔数, sum(金额) AS 合计 FROM 订单 GROUP BY 1, 2 ORDER BY 3 DESC"})[0]


def op_distinct():
    return call("POST", f"/ledger/datasets/{D}/distinct", None, body={**spec(), "column": random.choice(["店铺", "渠道", "状态"])})[0]


def op_sort_cursor():
    return call("POST", f"/ledger/datasets/{D}/cursor", None, body=spec(sort=[{"column": random.choice(["金额", "下单时间"]), "desc": random.random() < 0.5}]))[0]


# (名称, 函数, 权重)
OPS = [("游标取行", op_cursor_rows, 30), ("透视 店铺×月份", op_pivot_month, 10), ("透视 渠道×状态", op_pivot_small, 20), ("DSUMIFS×20", op_dsumifs, 20), ("SQL 分组", op_sql, 10), ("列值", op_distinct, 8), ("排序游标", op_sort_cursor, 2)]
names = [o[0] for o in OPS]
lat = {n: [] for n in names}
errors = {n: 0 for n in names}
busy = {n: 0 for n in names}
bad = []
lock = threading.Lock()
stop_at = time.time() + MINUTES * 60
alive = [True]


def worker():
    fns = {o[0]: o[1] for o in OPS}
    while time.time() < stop_at and alive[0]:
        name = random.choices(names, [o[2] for o in OPS])[0]
        t = time.time()
        try:
            status = fns[name]()
        except Exception as e:  # 连接失败等：记为错误
            status = f"EXC {e}"
        dt = time.time() - t
        with lock:
            if status == 200:
                lat[name].append(dt)
            elif status == 503:
                busy[name] += 1
            else:
                errors[name] += 1
                if len(bad) < 20:
                    bad.append((name, str(status), round(time.time() - t_start)))


samples = []  # (秒, MB)


def sampler():
    t0 = time.time()
    while time.time() < stop_at and alive[0]:
        m = rss_mb()
        if m is None:
            alive[0] = False
            with lock:
                bad.append(("服务进程", "已退出"))
            return
        samples.append((time.time() - t0, m))
        time.sleep(2)


ths = [threading.Thread(target=worker, daemon=True) for _ in range(WORKERS)] + [threading.Thread(target=sampler, daemon=True)]
t_start = time.time()
[t.start() for t in ths]
while time.time() < stop_at and alive[0]:
    time.sleep(30)
    with lock:
        done = sum(len(v) for v in lat.values())
        err = sum(errors.values())
    print(f"[{(time.time() - t_start) / 60:5.1f} min] 完成 {done}  错误 {err}  RSS {samples[-1][1]:.0f} MB" if samples else "…", flush=True)
[t.join(timeout=300) for t in ths]

pct = lambda v, p: sorted(v)[min(len(v) - 1, int(len(v) * p))] if v else None  # noqa: E731
result = {"minutes": MINUTES, "workers": WORKERS, "ops": {}, "errors": sum(errors.values()), "busy": sum(busy.values()), "problems": []}
for n in names:
    v = lat[n]
    result["ops"][n] = {"count": len(v), "errors": errors[n], "busy": busy[n], "p50_ms": round(statistics.median(v) * 1000) if v else None, "p95_ms": round(pct(v, 0.95) * 1000) if v else None, "max_ms": round(max(v) * 1000) if v else None}
if samples:
    warm_end = samples[-1][0] * WARMUP_FRACTION
    base = max((m for t, m in samples if t <= warm_end), default=samples[0][1])
    final = max((m for t, m in samples if t >= samples[-1][0] * 0.8), default=samples[-1][1])
    result["rss_mb"] = {"after_warmup_peak": round(base), "last_20pct_peak": round(final), "overall_peak": round(max(m for _, m in samples))}
    if final > base * 1.6 + 500:
        result["problems"].append(f"内存持续增长：预热后峰值 {base:.0f} MB → 最后 20% 时段峰值 {final:.0f} MB")
if errors and sum(errors.values()):
    result["problems"].append(f"{sum(errors.values())} 个请求出错（非 200 / 503）：{bad[:5]}")
if not alive[0]:
    result["problems"].append("服务进程在测试期间退出")
if sum(len(v) for v in lat.values()) == 0:
    result["problems"].append("没有任何成功请求")
json.dump(result, open(OUT, "w"), ensure_ascii=False, indent=2)
print(json.dumps(result, ensure_ascii=False, indent=2))
sys.exit(1 if result["problems"] else 0)
