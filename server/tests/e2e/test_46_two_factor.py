# env: LEDGER_AUTH_MODE=local LEDGER_ADMIN_USER=admin LEDGER_ADMIN_PASSWORD=Admin-pass-1
"""两步验证（TOTP）：设置 → 启用（得到恢复码）→ 登录要动态码 → 重放被拒 → 恢复码 → 失败锁定 → 停用 / 管理员重置

动态码用 Python 独立实现（hmac + sha1 + base32），与服务端 Rust 实现互相印证。
"""
from lib import *  # noqa: F403
import base64
import hashlib
import hmac
import json
import struct
import urllib.request


def totp(secret_b32, offset=0, t=None):
    key = base64.b32decode(secret_b32.upper() + "=" * (-len(secret_b32) % 8))
    step = int((t if t is not None else time.time()) // 30) + offset
    h = hmac.new(key, struct.pack(">Q", step), hashlib.sha1).digest()
    o = h[19] & 15
    return f"{(struct.unpack('>I', h[o:o + 4])[0] & 0x7fffffff) % 1000000:06d}"


def login(username, password, code=None):
    body = {"username": username, "password": password}
    if code is not None:
        body["code"] = code
    return call("POST", "/ledger/auth/login", body=body)


def bearer(token, method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(BASE + path, data=data, method=method, headers={"Authorization": f"Bearer {token}", **({"Content-Type": "application/json"} if data else {})})  # noqa: F405
    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            return resp.status, json.loads(resp.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read())


s, r = login("admin", "Admin-pass-1")
ADMIN = r["data"]["token"]
for u in ("alice", "bob"):
    call("POST", "/ledger/accounts", ADMIN, body={"username": u, "name": u.title(), "password": f"{u}-pass-1"})
ALICE = login("alice", "alice-pass-1")[1]["data"]["token"]

# ── 设置与启用 ──
s, r = call("GET", "/ledger/auth/2fa", ALICE)
check("一开始没有启用", s == 200 and r["data"] == {"enabled": False, "recoveryLeft": 0}, (s, r))
s, r = call("POST", "/ledger/auth/2fa/setup", ALICE)
SECRET = r["data"]["secret"] if s == 200 else ""
check("开始设置：返回 Base32 密钥和 otpauth 地址", s == 200 and len(SECRET) == 32 and r["data"]["uri"].startswith("otpauth://totp/Gridlake:alice?secret=" + SECRET), (s, r))
s, r = login("alice", "alice-pass-1")
check("设置了但没确认：登录不受影响", s == 200 and r["data"]["token"] and not r["data"].get("needCode"), (s, r))
s, r = call("POST", "/ledger/auth/2fa/enable", ALICE, body={"code": "000000" if totp(SECRET) != "000000" else "111111"})
check("确认码不对：不能启用", s == 400 and "验证码不正确" in msg((s, r)), (s, r))
enable_step_code = totp(SECRET)
s, r = call("POST", "/ledger/auth/2fa/enable", ALICE, body={"code": enable_step_code})
CODES = r["data"]["recoveryCodes"] if s == 200 else []
check("用验证器里的 6 位数字启用，得到 10 个不重复的恢复码", s == 200 and len(CODES) == 10 and len(set(CODES)) == 10 and all(len(c) == 11 for c in CODES), (s, r))
s, r = call("GET", "/ledger/auth/2fa", ALICE)
check("状态：已启用、恢复码 10 个", r["data"] == {"enabled": True, "recoveryLeft": 10}, r)
s, r = call("POST", "/ledger/auth/2fa/setup", ALICE)
check("启用后不能直接重新生成密钥", s == 400 and "已经启用" in msg((s, r)), (s, r))
s, r = call("GET", "/ledger/accounts", ADMIN)
check("管理员的账号列表里标出 alice 启用了两步验证", {a["username"]: a["twoFactor"] for a in r["data"]} == {"admin": False, "alice": True, "bob": False}, r)

# ── 登录 ──
s, h, body = call_full("POST", "/ledger/auth/login", body={"username": "alice", "password": "alice-pass-1"})
check("只给密码：要求验证码，没有令牌也没有会话 cookie", s == 200 and body["data"].get("needCode") is True and not body["data"]["token"] and "set-cookie" not in h, (s, h, body))
s, r = login("alice", "wrong-pass-1")
check("密码错了：照旧只说用户名或密码不正确（不泄露是否启用两步验证）", s == 400 and msg((s, r)) == "用户名或密码不正确", (s, r))
s, r = login("alice", "alice-pass-1", enable_step_code)
check("启用时用过的码不能重放", s == 400 and "验证码不正确" in msg((s, r)), (s, r))
s, r = login("alice", "alice-pass-1", "abcdef")
check("格式不对的码被拒绝", s == 400, (s, r))
code = totp(SECRET, 1)
s, r = login("alice", "alice-pass-1", code)
check("下一个时间步的码可以登录", s == 200 and r["data"]["token"] and r["data"]["user"]["id"] == "alice", (s, r))
TOKEN2 = r["data"]["token"] if s == 200 else ""
s, r = call("GET", "/ledger/me", TOKEN2)
check("拿到的会话可用", s == 200 and r["data"]["user"]["id"] == "alice", (s, r))
s, r = login("alice", "alice-pass-1", code)
check("同一个码只能用一次", s == 400, (s, r))
s, r = login("alice", "alice-pass-1", CODES[0])
check("恢复码可以登录", s == 200 and r["data"]["token"], (s, r))
s, r = login("alice", "alice-pass-1", CODES[0].upper().replace("-", ""))
check("恢复码用过就作废（写法宽松：大小写、连字符）", s == 400, (s, r))
s, r = call("GET", "/ledger/auth/2fa", ALICE)
check("恢复码还剩 9 个", r["data"]["recoveryLeft"] == 9, r)

# ── 个人访问令牌不能改两步验证 ──
s, r = call("POST", "/ledger/tokens", ALICE, body={"name": "ci"})
PAT = r["data"]["token"]
s, r = bearer(PAT, "POST", "/ledger/auth/2fa/setup")
check("访问令牌不能设置两步验证", s == 403, (s, r))
s, r = bearer(PAT, "GET", "/ledger/me")
check("访问令牌本身不受两步验证影响", s == 200 and r["data"]["user"]["id"] == "alice", (s, r))

# ── 停用 ──
s, r = call("POST", "/ledger/auth/2fa/disable", ALICE, body={"password": "wrong-pass-1", "code": CODES[1]})
check("停用要密码：密码错了被拒绝", s == 400 and "密码不正确" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/auth/2fa/disable", ALICE, body={"password": "alice-pass-1", "code": "abcdef"})
check("停用要验证码：码不对被拒绝", s == 400, (s, r))
s, r = call("POST", "/ledger/auth/2fa/disable", ALICE, body={"password": "alice-pass-1", "code": CODES[1]})
check("密码 + 恢复码可以停用", s == 200, (s, r))
s, r = login("alice", "alice-pass-1")
check("停用后只要密码", s == 200 and r["data"]["token"] and not r["data"].get("needCode"), (s, r))

# ── 失败锁定 + 管理员重置 ──
BOB = login("bob", "bob-pass-1")[1]["data"]["token"]
BS = call("POST", "/ledger/auth/2fa/setup", BOB)[1]["data"]["secret"]
check("bob 启用", call("POST", "/ledger/auth/2fa/enable", BOB, body={"code": totp(BS)})[0] == 200)
for _ in range(5):
    login("bob", "bob-pass-1", "999999")
s, r = login("bob", "bob-pass-1", totp(BS, 1))
check("连续输错 5 次验证码后锁定：即使这次对了也要等", s == 400 and "失败次数过多" in msg((s, r)), (s, r))
s, r = call("PUT", "/ledger/accounts/bob", ADMIN, body={"resetTotp": True})
check("管理员重置 bob 的两步验证", s == 200 and r["data"]["twoFactor"] is False, (s, r))
s, r = call("GET", "/ledger/me", BOB)
check("重置后 bob 已登录的会话失效", r["data"]["user"] is None, r)
s, r = call("GET", "/ledger/audit?pageSize=100", ADMIN)
rows = r["data"]["items"] if isinstance(r.get("data"), dict) else r.get("data", [])
acts = {x.get("action") for x in rows}
check("设置 / 启用 / 停用都记入审计", {"开始设置两步验证", "启用两步验证", "停用两步验证"} <= acts, acts)
s, r = call("PUT", "/ledger/accounts/alice", ALICE, body={"resetTotp": True})
check("普通用户不能重置别人的（管理员接口）", s == 403, (s, r))

finish()
