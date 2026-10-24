# env: LEDGER_AUTH_MODE=local LEDGER_ADMIN_USER=admin LEDGER_ADMIN_PASSWORD=Admin-pass-1
"""限定数据集的访问令牌：只能读写被限定的数据集（取行 / 统计 / 聚合 / 透视 / 导出 / 写入），其他数据集和其他接口一律 403；
和只读令牌可以叠加；数据集列表只给它限定的；按名称请求（公式下推）也认；吊销立即失效"""
from lib import *  # noqa: F403
import json
import urllib.parse
import urllib.request


def login(username, password):
    s, r = call("POST", "/ledger/auth/login", body={"username": username, "password": password})
    assert s == 200, (s, r)
    return r["data"]["token"]


def bearer(token, method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(BASE + path, data=data, method=method, headers={"Authorization": f"Bearer {token}", **({"Content-Type": "application/json"} if data else {})})  # noqa: F405
    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            return resp.status, json.loads(resp.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read())


ADMIN = login("admin", "Admin-pass-1")
call("POST", "/ledger/accounts", ADMIN, body={"username": "alice", "name": "Alice", "password": "alice-pass-1"})
ALICE = login("alice", "alice-pass-1")


def dataset(name):
    s, r = call("POST", "/ledger/datasets/input", ALICE, body={"name": name, "columns": [{"title": "单号", "kind": "text"}, {"title": "金额", "kind": "money"}], "primaryKey": ["单号"]})
    assert s == 200, (s, r)
    ds = r["data"]
    s, r = call("POST", f"/ledger/datasets/{ds}/writeback", ALICE, body={"mode": "upsert", "columns": ["单号", "金额"], "rows": [["1", 10], ["2", 20]]})
    assert s == 200, (s, r)
    return ds


ORDERS, SECRET = dataset("订单"), dataset("机密")
CUR = {"filters": [], "keyword": None, "sort": [], "rule": None}

# ── 创建与校验 ──
s, r = call("POST", "/ledger/tokens", ALICE, body={"name": "不存在", "datasets": ["ds_nope"]})
check("限定到不存在的数据集被拒", s == 400 and "不存在" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/tokens", ALICE, body={"name": "太多", "datasets": [f"ds_{i}" for i in range(51)]})
check("限定的数据集太多被拒", s == 400, (s, r))
s, r = call("POST", "/ledger/tokens", ALICE, body={"name": "只管订单", "datasets": [ORDERS, ORDERS]})
LIM = r["data"]["token"] if s == 200 else ""
check("创建：返回限定的数据集（去重、带名称）", s == 200 and r["data"]["datasets"] == [{"id": ORDERS, "name": "订单"}] and "datasets" not in r["data"]["token"], (s, r))
s, r = call("POST", "/ledger/tokens", ALICE, body={"name": "全部"})
FREE = r["data"]["token"]
check("不限定的令牌：datasets 为空", s == 200 and r["data"]["datasets"] == [], (s, r))
s, r = call("GET", "/ledger/tokens", ALICE)
check("列表里区分限定与不限定", {t["name"]: len(t["datasets"]) for t in r["data"]} == {"只管订单": 1, "全部": 0}, r)

# ── 被限定的数据集：读写都行 ──
s, r = bearer(LIM, "GET", "/ledger/datasets")
check("数据集列表只给限定的那一个", s == 200 and [d["id"] for d in r["data"]] == [ORDERS], (s, r))
s, r = bearer(FREE, "GET", "/ledger/datasets")
check("对照：不限定的令牌看得到全部", s == 200 and {d["id"] for d in r["data"]} >= {ORDERS, SECRET}, (s, r))
s, r = bearer(LIM, "POST", f"/ledger/datasets/{ORDERS}/cursor", CUR)
check("能打开游标", s == 200 and r["data"]["total"] == 2, (s, r))
cur = r["data"]["cursorId"] if s == 200 else ""
s, r = bearer(LIM, "GET", f"/ledger/datasets/{ORDERS}/rows?cursor={cur}&offset=0&limit=10")
check("能取行", s == 200 and len(r["data"]["rows"]) == 2, (s, r))
s, r = bearer(LIM, "POST", f"/ledger/datasets/{ORDERS}/aggregate", {"calls": [{"func": "sum", "column": "金额", "criteria": []}]})
check("能聚合（公式下推）", s == 200 and r["data"]["values"] == [30], (s, r))
s, r = bearer(LIM, "POST", f"/ledger/datasets/{urllib.parse.quote('订单')}/aggregate", {"calls": [{"func": "max", "column": "金额", "criteria": []}]})
check("按数据集名称请求同样认", s == 200 and r["data"]["values"] == [20], (s, r))
s, r = bearer(LIM, "POST", f"/ledger/datasets/{ORDERS}/pivot", {"rows": [], "values": [{"column": "金额", "agg": "sum"}]})
check("能做透视", s == 200, (s, r))
s, r = bearer(LIM, "POST", f"/ledger/datasets/{ORDERS}/writeback", {"mode": "upsert", "columns": ["单号", "金额"], "rows": [["3", 30]]})
check("能写入被限定的数据集（令牌不是只读的）", s == 200 and r["data"]["inserted"] == 1, (s, r))

# ── 其他数据集、其他接口：全部 403 ──
denied = [
    ("POST", f"/ledger/datasets/{SECRET}/cursor", CUR), ("GET", f"/ledger/datasets/{SECRET}/rows?cursor=x&offset=0&limit=1", None),
    ("POST", f"/ledger/datasets/{SECRET}/aggregate", {"calls": []}), ("POST", f"/ledger/datasets/{urllib.parse.quote('机密')}/aggregate", {"calls": []}),
    ("POST", f"/ledger/datasets/{SECRET}/writeback", {"mode": "upsert", "columns": ["单号", "金额"], "rows": [["9", 9]]}),
    ("GET", f"/ledger/datasets/{SECRET}/export?cursor=x&format=csv", None),
    # 被限定的数据集上，登记表之外的操作也不行
    ("PUT", f"/ledger/datasets/{ORDERS}", {"name": "改名"}), ("DELETE", f"/ledger/datasets/{ORDERS}", None), ("POST", f"/ledger/datasets/{ORDERS}/calc", {}),
    ("POST", "/ledger/sql/run", {"sql": "SELECT * FROM 机密"}), ("GET", "/ledger/workbooks", None), ("POST", "/ledger/folders", {"name": "x", "parentId": None}),
    ("GET", "/ledger/audit", None), ("GET", "/ledger/accounts", None), ("GET", "/ledger/tokens", None), ("POST", "/ledger/tokens", {"name": "再造一个"}),
    ("POST", "/ledger/datasets/import/commit", {}),
]
bad = [(m, p.split("?")[0]) for m, p, b in denied if bearer(LIM, m, p, b)[0] != 403]
check("其他数据集和其他接口全部 403", not bad, bad)
s, r = bearer(LIM, "POST", f"/ledger/datasets/{SECRET}/cursor", CUR)
check("提示里说明是令牌限定了数据集", s == 403 and "限定了可访问的数据集" in msg((s, r)), (s, r))
s, r = bearer(FREE, "POST", f"/ledger/datasets/{SECRET}/cursor", CUR)
check("对照：不限定的令牌读得到「机密」", s == 200, (s, r))
s, r = bearer(LIM, "GET", "/ledger/me")
check("/me 照常可用", s == 200 and r["data"]["user"]["name"] == "Alice", (s, r))
s, r = call("GET", "/ledger/datasets", LIM)
check("放在 x-user-token 里同样受限", s == 200 and [d["id"] for d in r["data"]] == [ORDERS], (s, r))

# ── 与只读叠加 ──
s, r = call("POST", "/ledger/tokens", ALICE, body={"name": "只读订单", "readOnly": True, "datasets": [ORDERS]})
RO = r["data"]["token"]
check("只读 + 限定：glr_ 开头", s == 200 and RO.startswith("glr_"), (s, r))
s, r = bearer(RO, "POST", f"/ledger/datasets/{ORDERS}/aggregate", {"calls": [{"func": "sum", "column": "金额", "criteria": []}]})
check("只读 + 限定：能读订单", s == 200 and r["data"]["values"] == [60], (s, r))
s, r = bearer(RO, "POST", f"/ledger/datasets/{ORDERS}/writeback", {"mode": "upsert", "columns": ["单号", "金额"], "rows": [["4", 4]]})
check("只读 + 限定：不能写订单", s == 403, (s, r))
s, r = bearer(RO, "POST", f"/ledger/datasets/{SECRET}/aggregate", {"calls": []})
check("只读 + 限定：不能读机密", s == 403, (s, r))

# ── 数据集改名、吊销 ──
s, r = call("PUT", f"/ledger/datasets/{ORDERS}", ALICE, body={"name": "销售订单"})
check("改名", s == 200, (s, r))
time.sleep(31)  # 令牌身份缓存 30 秒；改名后限定范围跟着新名称走
s, r = bearer(LIM, "POST", f"/ledger/datasets/{urllib.parse.quote('销售订单')}/aggregate", {"calls": [{"func": "sum", "column": "金额", "criteria": []}]})
check("改名后按新名称请求能通过", s == 200 and r["data"]["values"] == [60], (s, r))
s, r = bearer(LIM, "POST", f"/ledger/datasets/{urllib.parse.quote('订单')}/aggregate", {"calls": []})
check("旧名称不再属于它（也不会被别的数据集借用）", s == 403, (s, r))
s, r = call("GET", "/ledger/tokens", ALICE)
check("列表显示当前名称", any(d["name"] == "销售订单" for t in r["data"] for d in t["datasets"]), r)
tid = next(t["id"] for t in r["data"] if t["name"] == "只管订单")
s, r = call("DELETE", f"/ledger/tokens/{tid}", ALICE)
s, r = bearer(LIM, "GET", "/ledger/datasets")
check("吊销后立即失效", s == 401, (s, r))
finish()
