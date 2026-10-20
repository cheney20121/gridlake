# env: LEDGER_AUTH_MODE=local LEDGER_ADMIN_USER=admin LEDGER_ADMIN_PASSWORD=Admin-pass-1 LEDGER_OIDC_ISSUER=http://127.0.0.1:19003 LEDGER_OIDC_CLIENT_ID=gridlake LEDGER_OIDC_CLIENT_SECRET=s3cret LEDGER_OIDC_REDIRECT_URL=http://127.0.0.1:18787/api/ledger/auth/oidc/callback LEDGER_OIDC_ADMIN_GROUP=gl-admins LEDGER_OIDC_NAME=Mock-SSO
"""单点登录（OIDC，授权码 + PKCE）：用模拟身份提供方（mock_oidc.py）走完整流程

浏览器的部分由测试扮演：不跟随跳转，自己带 cookie 访问每一跳，才能检查 state cookie、错误代码、会话 cookie。
"""
import atexit
import http.client
import os
import subprocess
import sys
import urllib.parse

from lib import *  # noqa: F403

HERE = os.path.dirname(os.path.abspath(__file__))
IDP = "127.0.0.1:19003"
srv = urllib.parse.urlparse(BASE)
idp = subprocess.Popen([sys.executable, os.path.join(HERE, "mock_oidc.py"), "19003"])
atexit.register(idp.kill)
for _ in range(50):
    try:
        http.client.HTTPConnection(IDP, timeout=2).request("GET", "/__last")
        break
    except OSError:
        time.sleep(0.1)


def raw(host, path, cookies=None, method="GET"):
    """不跟随跳转的请求：返回 (状态码, 响应头 dict(小写键，Set-Cookie 用 | 连接), 响应体)"""
    c = http.client.HTTPConnection(host, timeout=30)
    h = {"Cookie": "; ".join(f"{k}={v}" for k, v in (cookies or {}).items())} if cookies else {}
    c.request(method, path, headers=h)
    r = c.getresponse()
    hdrs = {}
    for k, v in r.getheaders():
        hdrs[k.lower()] = hdrs[k.lower()] + "|" + v if k.lower() in hdrs else v
    body = r.read()
    c.close()
    return r.status, hdrs, body


def jar(hdrs):
    """Set-Cookie → {名: 值}（Max-Age=0 的表示清除，值为空串）"""
    out = {}
    for sc in hdrs.get("set-cookie", "").split("|"):
        if "=" in sc:
            k, v = sc.split(";")[0].split("=", 1)
            out[k.strip()] = v
    return out


def idp_next(user, fault="", deny=False):
    raw(IDP, f"/__next?user={user}&fault={fault}&deny={'1' if deny else '0'}")


def sso(user, ret=None, fault="", deny=False, drop_cookie=False):
    """走一遍完整流程；返回 (最终跳转地址, 会话 cookie 或 None, 登录页给的错误代码或 None)"""
    idp_next(user, fault, deny)
    s, h, _ = raw(srv.netloc, srv.path + "/ledger/auth/oidc/login" + (f"?return={urllib.parse.quote(ret)}" if ret else ""))
    assert s == 302, (s, h)
    cookies = jar(h)
    s, h, _ = raw(IDP, h["location"].split(IDP, 1)[1])  # 在身份提供方「登录」，被送回回调地址
    assert s == 302, (s, h)
    back = urllib.parse.urlparse(h["location"])
    s, h, _ = raw(srv.netloc, back.path + "?" + back.query, {} if drop_cookie else {"gridlake_oidc": cookies["gridlake_oidc"]})
    assert s == 302, (s, h)
    loc = h["location"]
    err = urllib.parse.parse_qs(urllib.parse.urlparse(loc).query).get("sso_error", [None])[0]
    cleared = jar(h).get("gridlake_oidc") == ""
    return loc, jar(h).get("gridlake_session"), err, cleared


def me(session):
    return call("GET", "/ledger/me", headers={"Cookie": f"gridlake_session={session}"})[1]["data"]


# ── 入口 ──
s, r = call("GET", "/ledger/me")
check("/me 告诉前端有 SSO：按钮名字和入口", s == 200 and r["data"]["sso"] == {"name": "Mock-SSO", "loginPath": "/ledger/auth/oidc/login"}, r)

s, h, _ = raw(srv.netloc, srv.path + "/ledger/auth/oidc/login")
loc = urllib.parse.urlparse(h["location"])
q = urllib.parse.parse_qs(loc.query)
check("开始登录：302 到身份提供方，带授权码流程、PKCE(S256)、state、nonce、回调地址",
      s == 302 and loc.netloc == IDP and loc.path == "/authorize" and q["response_type"] == ["code"] and q["client_id"] == ["gridlake"]
      and q["code_challenge_method"] == ["S256"] and len(q["code_challenge"][0]) == 43 and len(q["state"][0]) >= 24 and len(q["nonce"][0]) >= 24
      and q["redirect_uri"] == [f"http://127.0.0.1:18787/api/ledger/auth/oidc/callback"] and "openid" in q["scope"][0], (s, h))
sc = h["set-cookie"]
check("state 写进 HttpOnly + SameSite=Lax 的 cookie，且与 URL 里的 state 一致",
      jar(h)["gridlake_oidc"] == q["state"][0] and "HttpOnly" in sc and "SameSite=Lax" in sc, sc)

# ── 首次登录：自动建账号 ──
loc, session, err, cleared = sso("zhang")
check("首次登录成功：回到首页、写会话 cookie、清掉 state cookie", loc == "/" and session and len(session) >= 40 and err is None and cleared, (loc, session, err))
u = me(session)["user"]
check("自动建了普通账号：用户名取自 preferred_username，姓名取自 name", u == {"id": "zhang", "name": "张三", "admin": False}, u)
last = json.loads(raw(IDP, "/__last")[2])
check("令牌请求：授权码模式、带 client_secret 与 redirect_uri", last == {"has_secret": True, "client_id": "gridlake", "grant_type": "authorization_code", "redirect_uri": "http://127.0.0.1:18787/api/ledger/auth/oidc/callback"}, last)
s, r = call("GET", "/ledger/workbooks", headers={"Cookie": f"gridlake_session={session}"})
check("会话和密码登录的一样好用：能访问业务接口", s == 200, (s, r))
s, r = call("POST", "/ledger/auth/login", body={"username": "zhang", "password": "anything-123"})
check("SSO 账号不能用密码登录", s == 400 and msg((s, r)) == "用户名或密码不正确", (s, r))
s, r = call("POST", "/ledger/auth/login", body={"username": "zhang", "password": "!sso"})
check("占位哈希也不能被当作密码", s == 400, (s, r))

# ── 再次登录：还是同一个账号 ──
loc, session2, err, _ = sso("zhang", ret="/excel/home")
check("再次登录：同一个账号，回到指定的站内页面", loc == "/excel/home" and me(session2)["user"]["id"] == "zhang", (loc, err))
check("两次登录是两个独立会话", session2 != session)
for bad in ["//evil.example.com/x", "https://evil.example.com", "/\\evil.example.com"]:
    loc, _, err, _ = sso("zhang", ret=bad)
    check(f"return 只能是站内路径：{bad}", loc == "/" and err is None, loc)

# ── 同名不同人 ──
loc, s3, err, _ = sso("zhang2")
check("身份提供方里另一个同名用户：不会顶替，建出新账号", me(s3)["user"]["id"] == "zhang-2", me(s3))

# ── 默认不绑定同名的本地账号 ──
s, r = call("POST", "/ledger/auth/login", body={"username": "admin", "password": "Admin-pass-1"})
ADMIN = r["data"]["token"]
s, r = call("POST", "/ledger/accounts", ADMIN, body={"username": "alice", "name": "Alice", "password": "alice-pass-1", "admin": True})
loc, s4, err, _ = sso("alice")
check("身份提供方里叫 alice 的人，不会拿到本地管理员 alice 的账号", me(s4)["user"] == {"id": "alice-2", "name": "Alice-SSO", "admin": False}, me(s4))

# ── 管理员组 ──
loc, s5, err, _ = sso("boss")
check("在管理员组里：自动是管理员", me(s5)["user"]["admin"] is True, me(s5))
s, r = call("GET", "/ledger/accounts", headers={"Cookie": f"gridlake_session={s5}"})
check("管理员能用管理接口", s == 200 and any(a["username"] == "boss" for a in r["data"]), (s, r))
loc, s6, err, _ = sso("carl")
check("id_token 里没有组时，用 userinfo 补上", me(s6)["user"]["admin"] is True, me(s6))
loc, s7, err, _ = sso("mailonly")
check("没有登录名声明：取邮箱 @ 之前", me(s7)["user"]["id"] == "wang.wu", me(s7))

# ── 失败：只给浏览器一个简短的错误代码，不回显细节 ──
cases = [
    ("nonce 对不上", dict(user="zhang", fault="badnonce"), "token"),
    ("id_token 过期", dict(user="zhang", fault="expired"), "token"),
    ("aud 不是本应用", dict(user="zhang", fault="wrongaud"), "token"),
    ("iss 不是配置的签发者", dict(user="zhang", fault="wrongiss"), "token"),
    ("响应里没有 id_token", dict(user="zhang", fault="noidtoken"), "token"),
    ("用户在身份提供方拒绝授权", dict(user="zhang", deny=True), "denied"),
    ("state cookie 缺失（换了浏览器 / 伪造的回调链接）", dict(user="zhang", drop_cookie=True), "expired"),
]
for name, kw, code in cases:
    loc, session, err, _ = sso(**kw)
    check(f"{name}：回登录页、错误代码 {code}、不开会话", loc == f"/login?sso_error={code}" and session is None, (loc, session))

# 授权码 / state 只能用一次
idp_next("zhang")
s, h, _ = raw(srv.netloc, srv.path + "/ledger/auth/oidc/login")
ck = jar(h)
s, h2, _ = raw(IDP, h["location"].split(IDP, 1)[1])
back = urllib.parse.urlparse(h2["location"])
s, h3, _ = raw(srv.netloc, back.path + "?" + back.query, ck)
check("第一次回调成功", jar(h3).get("gridlake_session"), h3)
s, h4, _ = raw(srv.netloc, back.path + "?" + back.query, ck)
check("同一个回调再来一次（重放）：被拒绝", h4["location"] == "/login?sso_error=expired" and not jar(h4).get("gridlake_session"), h4)
s, h5, _ = raw(srv.netloc, srv.path + "/ledger/auth/oidc/callback", ck)
check("没有 code / state 的回调：被拒绝", h5["location"] == "/login?sso_error=expired", h5)
s, h6, _ = raw(srv.netloc, srv.path + "/ledger/auth/oidc/callback?code=x&state=forged", {"gridlake_oidc": "forged"})
check("伪造的 state（cookie 也是伪造的）：被拒绝", h6["location"] == "/login?sso_error=expired", h6)

# ── 停用账号 ──
s, r = call("PUT", "/ledger/accounts/zhang", ADMIN, body={"disabled": True})
check("管理员停用 zhang", s == 200, (s, r))
loc, session, err, _ = sso("zhang")
check("账号停用后 SSO 也登录不了", loc == "/login?sso_error=disabled" and session is None, (loc, session))
check("停用时已有的会话作废", call("GET", "/ledger/me", headers={"Cookie": f"gridlake_session={session2}"})[1]["data"]["user"] is None)

# ── 退出登录 ──
s, r = call("POST", "/ledger/auth/logout", headers={"Cookie": f"gridlake_session={s5}", "x-requested-with": "XMLHttpRequest"})
check("退出登录", s == 200 and me(s5)["user"] is None, (s, r))

# ── 审计 ──
s, r = call("GET", "/ledger/audit?pageSize=100", ADMIN)
rows = r["data"]["items"] if isinstance(r.get("data"), dict) else r.get("data", [])
check("SSO 登录和失败都进了审计", any(x.get("action") == "登录" and x.get("userId") == "zhang" for x in rows) and any(x.get("action") == "登录失败" for x in rows), rows[:3])

idp.kill()
finish()
