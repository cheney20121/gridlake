import os, sys, time, json, subprocess, threading, urllib.request
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
    print(f"{ms:>10.0f} ms  {label}   [RSS {rss():.0f} MB, 峰值 {peak[0]:.0f} MB]", flush=True)
    return out

s, r = call("GET", "/ledger/datasets", None)
ds = [d for d in r["data"] if d["name"] == "订单"][0]; D = ds["id"]
print("行数", ds["rowCount"])
spec = lambda **k: {"filters": [], "keyword": None, "sort": [], "rule": None, **k}
def cursor(label, sp):
    c = timed(label + "：打开游标", lambda: call("POST", f"/ledger/datasets/{D}/cursor", None, body=sp)[1])
    return c["data"]
def rows(label, cur, off, n=200):
    return timed(f"{label}：取 {n} 行（位置 {off:,}）", lambda: call("GET", f"/ledger/datasets/{D}/rows?cursor={cur['cursorId']}&offset={off}&limit={n}", None)[1])

print("\n── 自然顺序")
c0 = cursor("无筛选", spec()); print("   总行数", c0["total"])
for off in (0, 50_000_000, 99_999_000): rows("无筛选", c0, off)
print("\n── 筛选")
c1 = cursor("店铺=店铺007", spec(filters=[{"op": "in", "column": "店铺", "values": ["店铺007"]}])); print("   命中", c1["total"]); rows("店铺筛选", c1, 0)
c2 = cursor("2026-09 且 状态=已退款", spec(filters=[{"op": "between", "column": "下单时间", "min": "2026-09-01", "max": "2026-09-30 23:59:59"}, {"op": "in", "column": "状态", "values": ["已退款"]}])); print("   命中", c2["total"])
c3 = cursor("金额 ≥ 1400", spec(filters=[{"op": "between", "column": "金额", "min": 1400, "max": None}])); print("   命中", c3["total"])
print("\n── 关键字搜索（仅前 500 万行有搜索文本）")
c4 = cursor("关键字 商品00042", spec(keyword="商品00042")); print("   命中", c4["total"])
print("\n── 排序（全量排序物化）")
c5 = cursor("按金额降序（冷）", spec(sort=[{"column": "金额", "desc": True}])); print("   总", c5["total"])
for off in (0, 50_000_000, 99_999_000): rows("金额降序", c5, off)
timed("按金额降序（缓存命中）", lambda: call("POST", f"/ledger/datasets/{D}/cursor", None, body=spec(sort=[{"column": "金额", "desc": True}])))
c6 = cursor("按下单时间升序（冷）", spec(sort=[{"column": "下单时间", "desc": False}])); rows("时间升序", c6, 70_000_000)
c7 = cursor("店铺筛选 + 金额降序", spec(filters=[{"op": "in", "column": "店铺", "values": ["店铺007"]}], sort=[{"column": "金额", "desc": True}])); rows("店铺+金额降序", c7, 100_000)
print("\n── 选区统计 / 列值")
timed("状态栏统计：前 500 万行的金额", lambda: call("POST", f"/ledger/datasets/{D}/stats", None, body={"cursorId": c0["cursorId"], "from": 0, "to": 5_000_000, "columns": ["金额"]}))
timed("状态栏统计：全部 1 亿行的金额", lambda: call("POST", f"/ledger/datasets/{D}/stats", None, body={"cursorId": c0["cursorId"], "from": 0, "to": 99_999_999, "columns": ["金额"]}))
timed("筛选下拉：店铺（300 个值）", lambda: call("POST", f"/ledger/datasets/{D}/distinct", None, body={**spec(), "column": "店铺"}))
timed("筛选下拉：商品（2 万个值）", lambda: call("POST", f"/ledger/datasets/{D}/distinct", None, body={**spec(), "column": "商品"}))
timed("筛选下拉：订单号搜索 'DY00000001'", lambda: call("POST", f"/ledger/datasets/{D}/distinct", None, body={**spec(), "column": "订单号", "search": "DY00000001"}))
print("\n── 透视")
P = lambda body: call("POST", f"/ledger/datasets/{D}/pivot", None, body=body)
r = timed("店铺 × 月份：金额合计（300 × 33）", lambda: P({"rows": [{"column": "店铺"}], "columns": [{"column": "下单时间", "granularity": "month"}], "values": [{"column": "金额", "agg": "sum"}]})); print("   ", r[0], "行" , len(r[1]["data"]["rows"]) if r[0]==200 else r[1])
timed("渠道 × 状态：笔数 + 金额", lambda: P({"rows": [{"column": "渠道"}], "columns": [{"column": "状态"}], "values": [{"agg": "count"}, {"column": "金额", "agg": "sum"}]}))
timed("商品前 10：金额合计（2 万分组）", lambda: P({"rows": [{"column": "商品"}], "values": [{"column": "金额", "agg": "sum"}], "top": 10}))
timed("月度：金额合计 + 同比", lambda: P({"rows": [{"column": "下单时间", "granularity": "month"}], "values": [{"column": "金额", "agg": "sum"}, {"column": "金额", "agg": "sum", "show": "yoy"}]}))
timed("店铺：商品去重数 + 订单号去重数（1 亿）", lambda: P({"rows": [{"column": "店铺"}], "values": [{"column": "商品", "agg": "count_distinct"}, {"column": "订单号", "agg": "count_distinct"}]}))
timed("店铺：金额中位数 / P95", lambda: P({"rows": [{"column": "店铺"}], "values": [{"column": "金额", "agg": "median"}, {"column": "金额", "agg": "p95"}]}))
print("\n── 公式下推 / SQL")
calls = [{"func": "sum", "column": "金额", "criteria": [{"column": "店铺", "value": f"店铺{i:03d}"}]} for i in range(100)]
timed("DSUMIFS 批量 100 个（按店铺）", lambda: call("POST", f"/ledger/datasets/{D}/aggregate", None, body={"calls": calls}))
timed("DSUMIFS 单个（店铺 + 状态）", lambda: call("POST", f"/ledger/datasets/{D}/aggregate", None, body={"calls": [{"func": "sum", "column": "金额", "criteria": [{"column": "店铺", "value": "店铺007"}, {"column": "状态", "value": "已完成"}]}]}))
timed("SQL：渠道 × 状态 分组", lambda: call("POST", "/ledger/sql/run", None, body={"sql": "SELECT 渠道, 状态, count(*) AS 笔数, sum(金额) AS 合计 FROM 订单 GROUP BY 1, 2 ORDER BY 3 DESC"}))
timed("SQL：店铺内金额排名（窗口函数，取前 5）", lambda: call("POST", "/ledger/sql/run", None, body={"sql": "SELECT * FROM (SELECT 店铺, 订单号, 金额, row_number() OVER (PARTITION BY 店铺 ORDER BY 金额 DESC) AS rn FROM 订单) WHERE rn <= 5 ORDER BY 店铺, rn"}))
print("\n── 导出 / 备份")
def export():
    t = time.time(); req = urllib.request.Request(f"http://127.0.0.1:18950/api/ledger/datasets/{D}/export?cursor={c1['cursorId']}&format=csv"); n = 0
    with urllib.request.urlopen(req, timeout=600) as resp:
        while chunk := resp.read(1 << 20): n += len(chunk)
    return n
n = timed("导出店铺 007 的全部行（CSV）", export); print("   字节", n)
t0 = time.time(); s, r = call("POST", "/admin/backup", None, body={}); print(f"{(time.time()-t0)*1000:>10.0f} ms  整库备份 -> {s} {json.dumps(r, ensure_ascii=False)[:200] if isinstance(r, dict) else r}")
print("\n── 并发：8 个透视同时请求")
def one(i): return P({"rows": [{"column": "店铺"}], "columns": [{"column": "下单时间", "granularity": "month"}], "values": [{"column": "金额", "agg": "sum"}], "filters": [{"op": "in", "column": "渠道", "values": [["直播", "商品卡", "短视频", "自然", "达人"][i % 5]]}]})
t = time.time(); ths = [threading.Thread(target=one, args=(i,)) for i in range(8)]; [x.start() for x in ths]; [x.join() for x in ths]
print(f"{(time.time()-t)*1000:>10.0f} ms  8 个并发透视全部完成 [峰值 RSS {peak[0]:.0f} MB]")
print("\n峰值 RSS", round(peak[0]), "MB")
