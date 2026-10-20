# env: LEDGER_AUTH_MODE=local LEDGER_ADMIN_USER=admin LEDGER_ADMIN_PASSWORD=Admin-pass-1
"""只读访问令牌（glr_…）：能读、能做计算类查询，写操作一律 403；前缀是令牌的一部分，改了就认不出；吊销立即失效"""
from lib import *  # noqa: F403
import json
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
s, r = call("POST", "/ledger/accounts", ADMIN, body={"username": "alice", "name": "Alice", "password": "alice-pass-1"})
ALICE = login("alice", "alice-pass-1")

s, r = call("POST", "/ledger/tokens", ALICE, body={"name": "报表", "readOnly": True})
d = r["data"] if s == 200 else {}
RO = d.get("token", "")
check("创建只读令牌：glr_ 开头、列表里标为只读", s == 200 and RO.startswith("glr_") and d["readOnly"] is True and d["prefix"] == RO[:8], (s, r))
s, r = call("POST", "/ledger/tokens", ALICE, body={"name": "完全"})
FULL = r["data"]["token"]
check("不勾只读就是普通令牌", s == 200 and FULL.startswith("glp_") and r["data"]["readOnly"] is False, (s, r))
s, r = call("GET", "/ledger/tokens", ALICE)
check("列表里区分两种", s == 200 and {t["name"]: t["readOnly"] for t in r["data"]} == {"报表": True, "完全": False}, r)

# ── 读 ──
for how, tok in [("Bearer", RO)]:
    s, r = bearer(tok, "GET", "/ledger/workbooks")
    check("只读令牌能读：列工作簿", s == 200, (s, r))
    s, r = bearer(tok, "GET", "/ledger/me")
    check("/me 认出是 alice", s == 200 and r["data"]["user"]["name"] == "Alice", (s, r))
    s, r = bearer(tok, "POST", "/ledger/sql/run", {"sql": "SELECT 1 AS x"})
    check("计算类的 POST 也能用：SQL 查询", s == 200 and r["data"]["rows"] == [[1]], (s, r))
s, r = call("GET", "/ledger/workbooks", RO)
check("放在 x-user-token 里同样是只读令牌", s == 200, (s, r))

# ── 写 ──
S_FOLDER = {"name": "只读不该建出来", "parentId": None}
s, r = bearer(FULL, "POST", "/ledger/folders", {"name": "完全令牌建的", "parentId": None})
check("对照：完全权限的令牌能建文件夹", s == 200, (s, r))
cases = [
    ("POST", "/ledger/folders", S_FOLDER),
    ("POST", "/ledger/workbooks", {"name": "x"}),
    ("PUT", "/ledger/folders/f1", {"name": "y"}),
    ("DELETE", "/ledger/folders/f1", None),
    ("POST", "/ledger/tokens", {"name": "再造一个"}),
    ("POST", "/ledger/auth/logout", None),
    ("POST", "/ledger/auth/password", {"oldPassword": "alice-pass-1", "newPassword": "alice-pass-2"}),
    ("POST", "/ledger/datasets/input", {"name": "z", "columns": []}),
]
for m, p, b in cases:
    s, r = bearer(RO, m, p, b)
    check(f"只读令牌不能 {m} {p}：403 并说明原因", s == 403 and "只读" in r["msg"], (s, r))
s, r = call("GET", "/ledger/folders", ALICE)
names = [f["name"] for f in r["data"]]
check("被拒绝的写操作没有生效", "只读不该建出来" not in names and "完全令牌建的" in names, names)
s, r = call("POST", "/ledger/folders", ALICE, body={"name": "会话照常能写", "parentId": None})
check("同一个账号用登录会话照常能写", s == 200, (s, r))
s, r = call("POST", "/ledger/auth/password", RO, body={"oldPassword": "alice-pass-1", "newPassword": "alice-pass-2"})
check("改密码不能用只读令牌（x-user-token 方式也一样）", s == 403, (s, r))
s, r = call("POST", "/ledger/auth/login", body={"username": "alice", "password": "alice-pass-1"})
check("密码没被改", s == 200, (s, r))

# ── 冒充 ──
forged = "glp_" + RO[4:]
s, r = bearer(forged, "GET", "/ledger/workbooks")
check("把 glr_ 改成 glp_ 冒充完全权限：认不出（401）", s == 401, (s, r))
s, r = bearer("glr_" + "x" * 43, "GET", "/ledger/workbooks")
check("乱写的 glr_ 令牌：401", s == 401, (s, r))

# ── 吊销 ──
pid = [t["id"] for t in call("GET", "/ledger/tokens", ALICE)[1]["data"] if t["readOnly"]][0]
s, r = call("DELETE", f"/ledger/tokens/{pid}", ALICE, headers={"x-requested-with": "XMLHttpRequest"})
check("吊销只读令牌", s == 200, (s, r))
s, r = bearer(RO, "GET", "/ledger/workbooks")
check("吊销后立即失效", s == 401, (s, r))

finish()
