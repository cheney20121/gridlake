# env: LEDGER_AUTH_MODE=local LEDGER_ADMIN_USER=admin LEDGER_ADMIN_PASSWORD=Admin-pass-1
"""个人访问令牌（PAT）：创建（只显示一次）、Bearer / x-user-token 两种用法、权限与账号一致、吊销、过期、停用账号、不能用令牌管理令牌"""
from lib import *  # noqa: F403
import json
import urllib.request

def login(username, password):
    s, r = call("POST", "/ledger/auth/login", body={"username": username, "password": password})
    assert s == 200, (s, r)
    return r["data"]["token"]

def bearer(token, method, path, body=None):
    """用 Authorization: Bearer 调接口"""
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(BASE + path, data=data, method=method, headers={"Authorization": f"Bearer {token}", **({"Content-Type": "application/json"} if data else {})})  # noqa: F405
    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            return resp.status, json.loads(resp.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read())

ADMIN = login("admin", "Admin-pass-1")
s, r = call("POST", "/ledger/accounts", ADMIN, body={"username": "alice", "name": "Alice", "password": "alice-pass-1"})
check("建普通账号 alice", s == 200, (s, r))
ALICE = login("alice", "alice-pass-1")

# ── 创建 ──
s, r = call("GET", "/ledger/tokens", ALICE)
check("一开始没有令牌", s == 200 and r["data"] == [], (s, r))
s, r = call("POST", "/ledger/tokens", ALICE, body={"name": "  "})
check("名称不能为空", s == 400, (s, r))
s, r = call("POST", "/ledger/tokens", ALICE, body={"name": "CI", "days": 99999})
check("有效期超范围", s == 400, (s, r))
s, r = call("POST", "/ledger/tokens", ALICE, body={"name": "CI"})
d = r["data"] if s == 200 else {}
TOKEN = d.get("token", "")
check("创建：返回完整令牌（glp_ 开头），默认 90 天有效期", s == 200 and TOKEN.startswith("glp_") and len(TOKEN) >= 40 and d["expiresAt"] and not d["expired"], (s, r))
check("令牌的开头几位作为前缀展示", d.get("prefix") == TOKEN[:8], d)
s, r = call("GET", "/ledger/tokens", ALICE)
check("列表里有它，但不含令牌本身", s == 200 and len(r["data"]) == 1 and "token" not in r["data"][0] and TOKEN not in json.dumps(r), (s, r))
s, r = call("POST", "/ledger/tokens", ALICE, body={"name": "永久", "days": 0})
FOREVER = r["data"]["token"] if s == 200 else ""
check("days=0 表示永不过期", s == 200 and r["data"]["expiresAt"] is None, (s, r))

# ── 使用 ──
s, r = call("GET", "/ledger/me", TOKEN)
check("x-user-token 里放令牌：认出是 alice，不是管理员", s == 200 and r["data"]["user"]["name"] == "Alice" and not r["data"]["user"]["admin"], (s, r))
s, r = bearer(TOKEN, "GET", "/ledger/me")
check("Authorization: Bearer 也行", s == 200 and r["data"]["user"]["name"] == "Alice", (s, r))
s, r = bearer(TOKEN, "POST", "/ledger/workbooks?name=PAT-test", None)
check("权限与账号一致：能做普通用户能做的事（或至少没被当作未登录）", s != 401, (s, r))
s, r = bearer(TOKEN, "GET", "/ledger/accounts")
check("管理员接口：普通账号的令牌同样被拒（403）", s == 403, (s, r))
s, r = call("GET", "/ledger/tokens", ALICE)
check("用过之后记下最近使用时间", s == 200 and r["data"][-1 if r["data"][0]["name"] == "永久" else 0].get("lastUsedAt"), r)
s, r = bearer("glp_" + "x" * 43, "GET", "/ledger/workbooks")
check("乱写的令牌：401", s == 401, (s, r))

# 令牌是按账号隔离的
s, r = call("GET", "/ledger/tokens", ADMIN)
check("管理员看不到别人的令牌", s == 200 and r["data"] == [], (s, r))

# ── 不能用令牌管理令牌 ──
s, r = call("POST", "/ledger/tokens", TOKEN, body={"name": "偷偷再造一个"})
check("用令牌创建令牌：403", s == 403 and "不能用访问令牌" in msg((s, r)), (s, r))
s, r = bearer(TOKEN, "POST", "/ledger/tokens", {"name": "偷偷再造一个"})
check("用 Bearer 创建令牌：同样 403", s == 403, (s, r))
pat_id = call("GET", "/ledger/tokens", ALICE)[1]["data"][0]["id"]
s, r = call("DELETE", f"/ledger/tokens/{pat_id}", TOKEN)
check("用令牌吊销令牌：403", s == 403, (s, r))

# ── 吊销 ──
s, r = call("DELETE", f"/ledger/tokens/{pat_id}", ADMIN)
check("别人的令牌不能吊销", s == 400, (s, r))
tokens = call("GET", "/ledger/tokens", ALICE)[1]["data"]
ci = next(t for t in tokens if t["name"] == "CI")
s, r = call("DELETE", f"/ledger/tokens/{ci['id']}", ALICE)
check("本人吊销", s == 200, (s, r))
s, r = call("GET", "/ledger/workbooks", TOKEN)
check("吊销后立即失效（缓存也清了）", s == 401, (s, r))
s, r = call("GET", "/ledger/workbooks", FOREVER)
check("其他令牌不受影响", s == 200, (s, r))

# ── 停用账号 ──
s, r = call("GET", "/ledger/accounts", ADMIN)
alice_id = next(a["id"] for a in r["data"] if a["username"] == "alice")
s, r = call("PUT", f"/ledger/accounts/{alice_id}", ADMIN, body={"disabled": True})
check("停用账号", s == 200, (s, r))
s, r = call("GET", "/ledger/workbooks", FOREVER)
check("账号停用后，它的令牌立即失效", s == 401, (s, r))
s, r = call("PUT", f"/ledger/accounts/{alice_id}", ADMIN, body={"disabled": False})
s, r = call("GET", "/ledger/workbooks", FOREVER)
check("重新启用后令牌恢复", s == 200, (s, r))

# ── 数量上限 ──
ALICE = login("alice", "alice-pass-1")
made = 1
while True:
    s, r = call("POST", "/ledger/tokens", ALICE, body={"name": f"批量{made}", "days": 1})
    if s != 200:
        break
    made += 1
check("每个账号最多 20 个", made == 20 and s == 400 and "最多 20" in msg((s, r)), (made, s, r))

# ── 审计 ──
# 审计是后台批量写入的（攒最多 2 秒再写）：等到两条都写进去，最多 10 秒
def audit_items():
    s, r = call("GET", "/ledger/audit?action=%E8%AE%BF%E9%97%AE%E4%BB%A4%E7%89%8C&size=50", ADMIN)
    return s, r, (r["data"]["items"] if s == 200 and isinstance(r["data"], dict) and "items" in r["data"] else [])
for _ in range(20):
    s, r, items = audit_items()
    text = json.dumps(items, ensure_ascii=False)
    if "创建访问令牌" in text and "吊销访问令牌" in text:
        break
    time.sleep(0.5)
check("创建 / 吊销令牌进入操作审计", any("创建访问令牌" in json.dumps(i, ensure_ascii=False) for i in items) and any("吊销访问令牌" in json.dumps(i, ensure_ascii=False) for i in items), (s, str(r)[:300]))

finish()
