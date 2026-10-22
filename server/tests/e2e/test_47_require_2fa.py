# env: LEDGER_AUTH_MODE=local LEDGER_ADMIN_USER=admin LEDGER_ADMIN_PASSWORD=Admin-pass-1 LEDGER_REQUIRE_2FA=1
"""管理员要求两步验证（LEDGER_REQUIRE_2FA=1）：没开启的内置账号登录后只能设置两步验证，设置完才能用其他功能；访问令牌不受限"""
from lib import *  # noqa: F403
import base64
import hashlib
import hmac
import json
import struct
import urllib.request


def totp(secret_b32, offset=0):
    key = base64.b32decode(secret_b32.upper() + "=" * (-len(secret_b32) % 8))
    h = hmac.new(key, struct.pack(">Q", int(time.time() // 30) + offset), hashlib.sha1).digest()
    o = h[19] & 15
    return f"{(struct.unpack('>I', h[o:o + 4])[0] & 0x7fffffff) % 1000000:06d}"


def bearer(token, method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(BASE + path, data=data, method=method, headers={"Authorization": f"Bearer {token}", **({"Content-Type": "application/json"} if data else {})})  # noqa: F405
    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            return resp.status, json.loads(resp.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read())


s, r = call("POST", "/ledger/auth/login", body={"username": "admin", "password": "Admin-pass-1"})
check("没开两步验证的账号仍然可以用密码登录（要先能进去设置）", s == 200 and r["data"]["token"], (s, r))
ADMIN = r["data"]["token"]

s, r = call("GET", "/ledger/me", ADMIN)
check("/me 告诉前端：必须先设置两步验证", s == 200 and r["data"]["twoFactorRequired"] is True and r["data"]["user"]["id"] == "admin", (s, r))
for m, p in [("GET", "/ledger/workbooks"), ("GET", "/ledger/datasets"), ("GET", "/ledger/accounts"), ("POST", "/ledger/folders")]:
    s, r = call(m, p, ADMIN, body={"name": "x", "parentId": None} if m == "POST" else None)
    check(f"设置之前 {m} {p}：403 并说明原因", s == 403 and "两步验证" in msg((s, r)), (s, r))

# 设置两步验证所需的接口、退出、改密码照常可用
s, r = call("GET", "/ledger/auth/2fa", ADMIN)
check("两步验证状态可以查", s == 200 and r["data"]["enabled"] is False, (s, r))
s, r = call("POST", "/ledger/auth/2fa/setup", ADMIN)
SECRET = r["data"]["secret"]
check("可以开始设置", s == 200 and len(SECRET) == 32, (s, r))
s, r = call("POST", "/ledger/auth/2fa/enable", ADMIN, body={"code": totp(SECRET)})
check("启用成功", s == 200 and len(r["data"]["recoveryCodes"]) == 10, (s, r))
s, r = call("GET", "/ledger/me", ADMIN)
check("启用后 /me 不再要求（缓存立即失效）", r["data"].get("twoFactorRequired") in (None, False), r)
s, r = call("GET", "/ledger/workbooks", ADMIN)
check("启用后其他接口恢复可用", s == 200, (s, r))

# 新建的账号：登录后同样被要求
s, r = call("POST", "/ledger/accounts", ADMIN, body={"username": "alice", "name": "Alice", "password": "alice-pass-1"})
check("管理员建账号", s == 200, (s, r))
ALICE = call("POST", "/ledger/auth/login", body={"username": "alice", "password": "alice-pass-1"})[1]["data"]["token"]
s, r = call("GET", "/ledger/workbooks", ALICE)
check("新账号没开两步验证：被拦住", s == 403 and "两步验证" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/tokens", ALICE, body={"name": "ci"})
check("没开两步验证的账号也不能新建访问令牌（令牌接口不在放行范围）", s == 403, (s, r))

# 访问令牌是独立凭证：不受限制（脚本没法输验证码）
SA = call("POST", "/ledger/auth/2fa/setup", ALICE)[1]["data"]["secret"]
call("POST", "/ledger/auth/2fa/enable", ALICE, body={"code": totp(SA)})
s, r = call("POST", "/ledger/tokens", ALICE, body={"name": "ci"})
PAT = r["data"]["token"]
check("开启后可以建访问令牌", s == 200 and PAT.startswith("glp_"), (s, r))
s, r = call("POST", "/ledger/auth/2fa/disable", ALICE, body={"password": "alice-pass-1", "code": totp(SA, 1)})
check("alice 又停用了两步验证", s == 200, (s, r))
s, r = call("GET", "/ledger/workbooks", ALICE)
check("停用后会话又被拦住", s == 403, (s, r))
s, r = bearer(PAT, "GET", "/ledger/workbooks")
check("访问令牌不受要求限制，脚本照常能用", s == 200, (s, r))

# 管理员重置别人的两步验证：对方回到「必须先设置」
call("PUT", "/ledger/accounts/alice", ADMIN, body={"resetTotp": True})
s, r = call("POST", "/ledger/auth/login", body={"username": "alice", "password": "alice-pass-1"})
ALICE2 = r["data"]["token"]
s, r = call("GET", "/ledger/me", ALICE2)
check("重置后重新登录：仍然必须先设置", r["data"]["twoFactorRequired"] is True, r)

finish()
