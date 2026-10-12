"""客户端断开时中断只读查询（不让放弃的重查询在后台白白占满 CPU）

用一个故意算不完的 SQL（四表笛卡尔积）：客户端 1 秒就断开，服务端的「正在执行」计数应该很快回到 0；
断开后服务照常处理新请求；写操作不受影响。
"""
import socket
import threading
import urllib.request

from lib import *  # noqa: F403
for t in (A,): call("GET", "/ledger/me", t)

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
csv = "单号,金额\n" + "\n".join(f"O{i},{i}" for i in range(3000)) + "\n"
s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("慢查询.csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
p = r["data"]
s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": "create", "name": "慢查询", "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
check("造数据", s == 200, (s, r))

def active():
    return json.loads(urllib.request.urlopen(BASE + "/health", timeout=5).read())["data"]["activeQueries"]

check("空闲时没有正在执行的操作", active() == 0, active())

SLOW = {"sql": "SELECT count(*) FROM 慢查询 a, 慢查询 b, 慢查询 c, 慢查询 d WHERE a.金额 + b.金额 + c.金额 + d.金额 = -1"}
def fire_and_abandon(path, body, tok):
    """发出请求后 1 秒不等结果，直接断开连接"""
    host, port = BASE.split("//")[1].split("/")[0].split(":")
    data = json.dumps(body).encode()
    sk = socket.create_connection((host, int(port)), timeout=5)
    sk.sendall((f"POST /api{path} HTTP/1.1\r\nHost: {host}\r\nx-user-token: {tok}\r\nContent-Type: application/json\r\nContent-Length: {len(data)}\r\n\r\n").encode() + data)
    time.sleep(1)
    sk.close()

t0 = time.time()
fire_and_abandon("/ledger/sql/run", SLOW, A)
# 断开后应在几秒内回到 0（没有取消的话这条查询要跑很久）
deadline = time.time() + 15
while time.time() < deadline and active() != 0:
    time.sleep(0.2)
left = active()
check("客户端断开后，慢查询被中断（正在执行计数回到 0）", left == 0, f"仍有 {left} 个，{time.time() - t0:.1f}s")
check("中断在几秒内完成", time.time() - t0 < 12, time.time() - t0)

# 同时断开多个
for _ in range(4): fire_and_abandon("/ledger/sql/run", SLOW, A)
deadline = time.time() + 20
while time.time() < deadline and active() != 0:
    time.sleep(0.2)
check("同时放弃 4 个慢查询也都被中断", active() == 0, active())

# 服务照常
s, r = call("POST", "/ledger/sql/run", A, body={"sql": "SELECT count(*) AS n FROM 慢查询"})
check("断开之后新请求正常", s == 200 and r["data"]["rows"][0][0] == 3000, (s, r))
# 正常等待结果的查询不受影响
s, r = call("POST", "/ledger/sql/run", A, body={"sql": "SELECT count(*) AS n FROM 慢查询 a, 慢查询 b WHERE a.金额 < b.金额"})
check("正常等待的重一点的查询照常返回", s == 200 and r["data"]["rows"][0][0] == 3000 * 2999 // 2, (s, r))
check("跑完后计数回到 0", active() == 0, active())
# 写操作不在此列：导入在客户端断开后仍然完成（走的是 blocking，不会被中断）
s, r = call("GET", "/ledger/datasets", A)
check("数据集列表正常", any(d["name"] == "慢查询" for d in r["data"]), s)

# 近似去重计数（高基数列用，秒级）：与精确去重相差不超过 8%
dsid = [d["id"] for d in call("GET", "/ledger/datasets", A)[1]["data"] if d["name"] == "慢查询"][0]
def total(agg):
    s, r = call("POST", f"/ledger/datasets/{dsid}/pivot", A, body={"rows": [], "values": [{"column": "单号", "agg": agg}]})
    assert s == 200, (s, r)
    return r["data"]["rows"][0]["cells"][0][0]
exact, approx = total("count_distinct"), total("approx_distinct")
check("精确去重计数 = 3000", exact == 3000, exact)
check("近似去重计数与精确值相差不超过 8%", isinstance(approx, (int, float)) and abs(approx - 3000) <= 240, approx)
s, r = call("POST", f"/ledger/datasets/{dsid}/pivot", A, body={"rows": [], "values": [{"column": "单号", "agg": "approx_distinct"}]})
check("值的标签是「近似去重计数」", "近似去重计数" in json.dumps(r["data"], ensure_ascii=False), r["data"].get("values"))
finish()
