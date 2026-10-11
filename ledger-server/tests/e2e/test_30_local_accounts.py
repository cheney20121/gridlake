# env: LEDGER_AUTH_MODE=local LEDGER_ADMIN_USER=admin LEDGER_ADMIN_PASSWORD=Admin-pass-1
"""内置账号（LEDGER_AUTH_MODE=local）：首次启动建管理员、登录 / 退出、改密码、管理员管理账号、防暴力尝试、审计

登录后与网关模式同构：工作簿所有者、用户目录（审批人可选）、管理员接口权限都按登录账号。
"""
from lib import *  # noqa: F403

def login(username, password):
    return call("POST", "/ledger/auth/login", body={"username": username, "password": password})

# ── 首次启动 ──
s, r = call("GET", "/ledger/me")
check("未登录：要求登录、方式为 local、没有用户", s == 200 and r["data"]["authRequired"] and r["data"]["mode"] == "local" and r["data"]["user"] is None, r)
s, r = call("GET", "/ledger/workbooks")
check("未登录访问业务接口：401", s == 401, (s, r))

s, r = login("admin", "wrong-password")
check("密码错误：说明不区分用户名是否存在", s == 400 and msg((s, r)) == "用户名或密码不正确", (s, r))
s, r = login("nobody", "whatever-123")
check("用户名不存在：同样的说明", s == 400 and msg((s, r)) == "用户名或密码不正确", (s, r))
s, r = login("ADMIN", "Admin-pass-1")
check("首次启动按环境变量建了管理员；用户名不区分大小写", s == 200 and r["data"]["user"]["admin"] and r["data"]["days"] == 7 and len(r["data"]["token"]) >= 40, (s, r))
ADMIN = r["data"]["token"]
s, r = call("GET", "/ledger/me", ADMIN)
check("带令牌：/me 返回管理员", r["data"]["user"] == {"id": "admin", "name": "管理员", "admin": True}, r["data"])

# ── 管理员建账号 ──
s, r = call("POST", "/ledger/accounts", ADMIN, body={"username": "alice", "name": "Alice", "password": "alice-pass-1"})
check("新建普通账号", s == 200 and r["data"]["username"] == "alice" and not r["data"]["admin"], (s, r))
s, r = call("POST", "/ledger/accounts", ADMIN, body={"username": "bob", "name": "Bob", "password": "bob-pass-1", "admin": False})
check("再建一个", s == 200, (s, r))
s, r = call("POST", "/ledger/accounts", ADMIN, body={"username": "Alice", "password": "another-123"})
check("用户名重复（不区分大小写）", s == 400 and "已存在" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/accounts", ADMIN, body={"username": "x y", "password": "another-123"})
check("用户名不合规", s == 400 and "用户名" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/accounts", ADMIN, body={"username": "carol", "password": "short"})
check("密码太短", s == 400 and "至少 8 位" in msg((s, r)), (s, r))
s, r = call("GET", "/ledger/users?keyword=", ADMIN)
check("新账号建好就在用户目录里（可被选为审批人），不用先登录", {"Alice", "Bob"} <= {u["name"] for u in r["data"]}, r)

s, r = login("alice", "alice-pass-1"); ALICE = r["data"]["token"]
check("Alice 登录", s == 200 and not r["data"]["user"]["admin"], (s, r))
s, r = call("GET", "/ledger/accounts", ALICE)
check("普通账号不能管理账号（403）", s == 403, (s, r))
s, r = call("POST", "/ledger/workbooks?name=" + urllib.parse.quote("Alice 的表") + "&operator=" + urllib.parse.quote("冒充"), ALICE, raw=gzip.compress(b'{"sheetOrder":[],"sheets":{}}'))
check("Alice 新建工作簿：所有者是 Alice，操作人不能冒充", s == 200 and r["data"]["ownerId"] == "alice" and r["data"]["updatedBy"] == "Alice", (s, r))
WB = r["data"]["id"]
s, r = call("GET", f"/ledger/workbooks/{WB}/content", None)
check("没有令牌读不到", s == 401, s)

# ── 改密码 ──
s, r = login("alice", "alice-pass-1"); ALICE2 = r["data"]["token"]   # 另一台设备
s, r = call("POST", "/ledger/auth/password", ALICE, body={"oldPassword": "错的", "newPassword": "alice-pass-2"})
check("原密码不对", s == 400 and "原密码" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/auth/password", ALICE, body={"oldPassword": "alice-pass-1", "newPassword": "alice-pass-2"})
check("改密码", s == 200, (s, r))
s, r = call("GET", "/ledger/me", ALICE)
check("当前设备仍登录着", r["data"]["user"] and r["data"]["user"]["id"] == "alice", r["data"])
s, r = call("GET", "/ledger/workbooks", ALICE2)
check("其他设备上的登录失效（401）", s == 401, s)
s, r = login("alice", "alice-pass-1")
check("旧密码不能再登录", s == 400, (s, r))
s, r = login("alice", "alice-pass-2"); ALICE = r["data"]["token"]
check("新密码登录", s == 200, (s, r))

# ── 管理员停用 / 重置密码 / 设管理员 ──
s, r = call("PUT", "/ledger/accounts/alice", ADMIN, body={"disabled": True})
check("停用 Alice", s == 200 and r["data"]["disabled"], (s, r))
s, r = call("GET", "/ledger/workbooks", ALICE)
check("停用后已登录的令牌立即失效", s == 401, s)
s, r = login("alice", "alice-pass-2")
check("停用的账号不能登录", s == 400 and "停用" in msg((s, r)), (s, r))
s, r = call("PUT", "/ledger/accounts/alice", ADMIN, body={"disabled": False, "password": "alice-reset-1"})
check("启用并重置密码", s == 200 and not r["data"]["disabled"], (s, r))
s, r = login("alice", "alice-reset-1")
check("重置后的密码可以登录", s == 200, (s, r))
s, r = call("PUT", "/ledger/accounts/admin", ADMIN, body={"admin": False})
check("不能撤掉最后一个管理员", s == 400 and "至少要保留一个" in msg((s, r)), (s, r))
s, r = call("PUT", "/ledger/accounts/admin", ADMIN, body={"disabled": True})
check("不能停用自己 / 最后一个管理员", s == 400, (s, r))
s, r = call("PUT", "/ledger/accounts/bob", ADMIN, body={"admin": True, "name": "Bob 主管"})
check("把 Bob 设为管理员并改显示名", s == 200 and r["data"]["admin"] and r["data"]["name"] == "Bob 主管", (s, r))
s, r = login("bob", "bob-pass-1"); BOB = r["data"]["token"]
s, r = call("GET", "/ledger/accounts", BOB)
check("Bob 现在能管理账号", s == 200 and len(r["data"]) == 3, (s, r))
s, r = call("PUT", "/ledger/accounts/admin", BOB, body={"admin": False})
check("有两个管理员时可以撤掉其中一个", s == 200 and not r["data"]["admin"], (s, r))
s, r = call("GET", "/ledger/accounts", ADMIN)
check("admin 不再是管理员后立即失去管理权限", s == 403, s)

# ── 会话 cookie（浏览器登录）：HttpOnly，写操作要防跨站伪造 ──
s, h, r = call_full("POST", "/ledger/auth/login", body={"username": "bob", "password": "bob-pass-1"})
cookie = h.get("set-cookie", "")
check("登录时下发会话 cookie：HttpOnly、SameSite=Lax、整站有效、有效期与 days 一致", s == 200 and "webexcel_session=" in cookie and "HttpOnly" in cookie and "SameSite=Lax" in cookie
      and "Path=/" in cookie and f"Max-Age={r['data']['days'] * 86400}" in cookie, (s, h))
check("本机直连（没有 https 代理）时不加 Secure", "Secure" not in cookie, cookie)
check("cookie 里就是返回的令牌", cookie.split(";")[0] == "webexcel_session=" + r["data"]["token"], cookie)
CK = {"Cookie": cookie.split(";")[0]}
s, r = call("GET", "/ledger/me", headers=CK)
check("只带 cookie：读接口可以用", s == 200 and r["data"]["user"] and r["data"]["user"]["id"] == "bob", (s, r))
s, r = call("POST", "/ledger/folders", headers=CK, body={"name": "cookie 文件夹"})
check("只带 cookie 的写请求：缺 x-requested-with 被拒绝（防跨站请求伪造）", s == 403 and "x-requested-with" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/folders", headers={**CK, "x-requested-with": "XMLHttpRequest"}, body={"name": "cookie 文件夹"})
check("带上 x-requested-with 后写请求正常", s == 200, (s, r))
s, r = call("POST", "/ledger/folders", BOB, body={"name": "令牌文件夹"})
check("用 x-user-token 请求头（脚本调用）的写请求不受影响", s == 200, (s, r))
s, h, r = call_full("POST", "/ledger/auth/login", headers={"X-Forwarded-Proto": "https"}, body={"username": "bob", "password": "bob-pass-1"})
check("经 https 反向代理登录：cookie 加 Secure", s == 200 and "Secure" in h.get("set-cookie", ""), h)
s, h, r = call_full("POST", "/ledger/auth/logout", headers={**CK, "x-requested-with": "XMLHttpRequest"})
check("退出：清除会话 cookie", s == 200 and "webexcel_session=;" in h.get("set-cookie", "") and "Max-Age=0" in h.get("set-cookie", ""), (s, h))
s, r = call("GET", "/ledger/me", headers=CK)
check("退出后同一个 cookie 已失效", s == 200 and r["data"]["user"] is None, (s, r))

# ── 防暴力尝试 ──
for _ in range(5):
    login("alice", "guess-guess")
s, r = login("alice", "alice-reset-1")
check("同一用户名连续失败 5 次后，正确的密码也暂时被拒绝", s == 400 and "失败次数过多" in msg((s, r)), (s, r))
s, r = login("bob", "bob-pass-1")
check("其他账号不受影响", s == 200, (s, r))

# ── 退出 ──
s, r = call("POST", "/ledger/auth/logout", BOB)
check("退出登录", s == 200, (s, r))
s, r = call("GET", "/ledger/workbooks", BOB)
check("退出后令牌失效", s == 401, s)

# ── 审计 ──
s, r = login("bob", "bob-pass-1"); BOB = r["data"]["token"]
time.sleep(2.5)   # 审计异步批量写入
s, r = call("GET", "/ledger/audit?size=200", BOB)
acts = [(i["userName"], i["action"], i["status"]) for i in r["data"]["items"]] if s == 200 else []
check("登录成功与失败都记入审计", ("alice", "登录失败", 400) in acts and ("bob", "登录", 200) in acts, acts[:20])
check("账号管理操作记入审计", any(a[1] == "新建账号" for a in acts) and any(a[1] == "修改账号" for a in acts), acts[:20])
check("改密码记入审计（不含密码）", any(a[1] == "修改密码" for a in acts), acts[:20])
finish()
