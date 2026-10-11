# env: LEDGER_HEAVY_QUERIES=1 LEDGER_HEAVY_WAIT_SECS=3
"""重计算并发闸门：同时只允许 1 个大计算，其余排队，排队超时返回「服务繁忙」；轻量请求不受影响

LEDGER_HEAVY_QUERIES=1、等待上限 3 秒（见第一行）。用一个算不完的 SQL 占住唯一的名额。
"""
import socket
import threading
import urllib.error
import urllib.request

from lib import *  # noqa: F403
call("GET", "/ledger/me", A)

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
csv = "单号,店铺,金额\n" + "\n".join(f"O{i},店{i % 5},{i}" for i in range(3000)) + "\n"
s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("闸门.csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
p = r["data"]
s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": "create", "name": "闸门", "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
D = r["data"]["datasetId"]

def health():
    return json.loads(urllib.request.urlopen(BASE + "/health", timeout=5).read())["data"]
h = health()
check("健康检查给出闸门配置", h["heavy"] == {"limit": 1, "running": 0, "waiting": 0}, h)
check("自动算出了内存上限", isinstance(h["memoryLimit"], str) and h["memoryLimit"].upper().endswith("MB"), h.get("memoryLimit"))

SLOW = {"sql": "SELECT count(*) FROM 闸门 a, 闸门 b, 闸门 c, 闸门 d WHERE a.金额 + b.金额 + c.金额 + d.金额 = -1"}
host, port = BASE.split("//")[1].split("/")[0].split(":")
def hold_slow():
    """发出慢查询并一直不断开，占住唯一的名额；返回 socket"""
    data = json.dumps(SLOW).encode()
    sk = socket.create_connection((host, int(port)), timeout=5)
    sk.sendall((f"POST /api/ledger/sql/run HTTP/1.1\r\nHost: {host}\r\nx-user-token: {A}\r\nContent-Type: application/json\r\nContent-Length: {len(data)}\r\n\r\n").encode() + data)
    return sk
sk = hold_slow()
for _ in range(50):
    if health()["heavy"]["running"] == 1: break
    time.sleep(0.1)
check("慢查询占住了名额", health()["heavy"]["running"] == 1, health())

# 第二个大计算排队，3 秒后返回「服务繁忙」（503）
res = {}
def pivot():
    t = time.time()
    res["r"] = call("POST", f"/ledger/datasets/{D}/pivot", A, body={"rows": [{"column": "店铺"}], "values": [{"column": "金额", "agg": "sum"}]})
    res["t"] = time.time() - t
th = threading.Thread(target=pivot); th.start()
time.sleep(1)
check("第二个大计算在排队", health()["heavy"]["waiting"] == 1, health())
# 排队期间轻量请求照常
t0 = time.time(); s1, r1 = call("GET", "/ledger/datasets", A); dt = time.time() - t0
check("排队期间数据集列表正常、很快", s1 == 200 and dt < 1.5, dt)
spec = {"filters": [], "keyword": None, "sort": [], "rule": None}
t0 = time.time(); s2, c = call("POST", f"/ledger/datasets/{D}/cursor", A, body=spec); dt = time.time() - t0
check("整表浏览（直通游标）不排队，立刻打开", s2 == 200 and c["data"]["direct"] is True and dt < 1.5, (s2, dt))
t0 = time.time(); s3, rr = call("GET", f"/ledger/datasets/{D}/rows?cursor={c['data']['cursorId']}&offset=100&limit=50", A); dt = time.time() - t0
check("取行不排队", s3 == 200 and len(rr["data"]["rows"]) == 50 and dt < 1.5, dt)
th.join()
s, r = res["r"]
check("排队超时返回 503「服务繁忙」", s == 503 and "服务繁忙" in msg((s, r)), (s, r))
check("等了约 3 秒", 2.5 <= res["t"] <= 6, res["t"])
check("排队计数回到 0", health()["heavy"]["waiting"] == 0, health())

# 带排序的游标也是大计算，同样排队
t0 = time.time(); s4, r4 = call("POST", f"/ledger/datasets/{D}/cursor", A, body={**spec, "sort": [{"column": "金额", "desc": True}]})
check("带排序的游标要排队，超时返回服务繁忙", s4 == 503 and time.time() - t0 >= 2.5, (s4, r4))
# 排队中的请求，客户端断开后退出队列（不会在名额空出来后白白执行）
def waiter():
    data = json.dumps({"rows": [{"column": "店铺"}], "values": [{"column": "金额", "agg": "sum"}]}).encode()
    sk2 = socket.create_connection((host, int(port)), timeout=5)
    sk2.sendall((f"POST /api/ledger/datasets/{D}/pivot HTTP/1.1\r\nHost: {host}\r\nx-user-token: {A}\r\nContent-Type: application/json\r\nContent-Length: {len(data)}\r\n\r\n").encode() + data)
    time.sleep(0.8); sk2.close()
waiter()
time.sleep(0.5)
check("排队中断开的请求退出了队列", health()["heavy"]["waiting"] == 0, health())

# 释放名额：断开慢查询 → 被中断 → 大计算恢复正常
sk.close()
for _ in range(100):
    if health()["heavy"]["running"] == 0: break
    time.sleep(0.1)
check("慢查询被中断，名额释放", health()["heavy"]["running"] == 0 and health()["activeQueries"] == 0, health())
s, r = call("POST", f"/ledger/datasets/{D}/pivot", A, body={"rows": [{"column": "店铺"}], "values": [{"column": "金额", "agg": "sum"}]})
check("名额释放后透视正常", s == 200 and len(r["data"]["rows"]) >= 5, (s, r if s != 200 else ""))
finish()
