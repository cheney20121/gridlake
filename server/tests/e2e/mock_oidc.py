"""模拟 OIDC 身份提供方（授权码 + PKCE）：只用于测试，id_token 不签名（Gridlake 直接信任经令牌接口拿到的 id_token）。

  GET  /.well-known/openid-configuration   发现文档
  GET  /authorize                          自动「登录」：按 /__next 指定的用户颁发授权码并 302 回 redirect_uri
  POST /token                              换令牌：校验 client_secret、redirect_uri、PKCE
  GET  /userinfo                           访问令牌 → 补充声明（组）
  GET  /__next?user=boss&fault=badnonce    指定下一次授权用谁登录、故意出什么错（见 FAULTS）；deny=1 表示用户拒绝授权
  GET  /__last                             最近一次令牌请求的参数（测试用来断言客户端认证方式）
  GET  /__shutdown                         退出

用法：python3 mock_oidc.py [端口，默认 19003]
"""
import base64
import hashlib
import json
import secrets
import sys
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 19003
ISSUER = f"http://127.0.0.1:{PORT}"
CLIENT_ID, CLIENT_SECRET = "gridlake", "s3cret"

USERS = {
    "zhang": {"sub": "s1", "preferred_username": "zhang", "name": "张三", "email": "zhang@corp.test"},
    # 另一个人，登录名也叫 zhang
    "zhang2": {"sub": "s2", "preferred_username": "zhang", "name": "张三（另一个）"},
    "boss": {"sub": "s3", "preferred_username": "boss", "name": "老板", "groups": ["gl-admins"]},
    # 组只在 userinfo 里，id_token 里没有
    "carl": {"sub": "s4", "preferred_username": "carl", "name": "Carl", "_userinfo": {"groups": ["gl-admins"]}},
    # 没有 preferred_username：回退到邮箱 @ 之前
    "mailonly": {"sub": "s5", "email": "wang.wu@corp.test"},
    "alice": {"sub": "s6", "preferred_username": "alice", "name": "Alice-SSO"},
}
# 故意出错的方式
FAULTS = {"badnonce", "expired", "wrongaud", "wrongiss", "noidtoken"}

state = {"next": {"user": "zhang", "fault": "", "deny": False}, "codes": {}, "tokens": {}, "last": {}}
lock = threading.Lock()


def b64(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def id_token(user: dict, nonce: str, fault: str) -> str:
    now = int(time.time())
    claims = {k: v for k, v in user.items() if not k.startswith("_")}
    claims.update({"iss": ISSUER, "aud": CLIENT_ID, "iat": now, "exp": now + 300, "nonce": nonce})
    if fault == "badnonce":
        claims["nonce"] = "someone-elses-nonce"
    elif fault == "expired":
        claims["exp"] = now - 3600
    elif fault == "wrongaud":
        claims["aud"] = "another-app"
    elif fault == "wrongiss":
        claims["iss"] = "https://evil.example.com"
    return ".".join([b64(b'{"alg":"none"}'), b64(json.dumps(claims).encode()), "sig"])


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _json(self, code, obj):
        payload = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def _redirect(self, to):
        self.send_response(302)
        self.send_header("Location", to)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_GET(self):
        u = urllib.parse.urlparse(self.path)
        q = {k: v[0] for k, v in urllib.parse.parse_qs(u.query).items()}
        if u.path == "/__shutdown":
            self.send_response(200)
            self.send_header("Content-Length", "0")
            self.end_headers()
            threading.Thread(target=self.server.shutdown, daemon=True).start()
        elif u.path == "/.well-known/openid-configuration":
            self._json(200, {
                "issuer": ISSUER, "authorization_endpoint": ISSUER + "/authorize", "token_endpoint": ISSUER + "/token",
                "userinfo_endpoint": ISSUER + "/userinfo", "token_endpoint_auth_methods_supported": ["client_secret_post"],
            })
        elif u.path == "/__next":
            with lock:
                state["next"] = {"user": q.get("user", "zhang"), "fault": q.get("fault", ""), "deny": q.get("deny") == "1"}
            self._json(200, {"ok": True})
        elif u.path == "/__last":
            self._json(200, state["last"])
        elif u.path == "/authorize":
            if q.get("response_type") != "code" or q.get("client_id") != CLIENT_ID or q.get("code_challenge_method") != "S256" or not q.get("code_challenge") or not q.get("state") or not q.get("nonce"):
                return self._json(400, {"error": "invalid_request", "got": q})
            sep = "&" if "?" in q["redirect_uri"] else "?"
            with lock:
                nxt = dict(state["next"])
            if nxt["deny"]:
                return self._redirect(q["redirect_uri"] + sep + urllib.parse.urlencode({"error": "access_denied", "error_description": "user said no", "state": q["state"]}))
            code = secrets.token_urlsafe(16)
            with lock:
                state["codes"][code] = {"user": nxt["user"], "fault": nxt["fault"], "nonce": q["nonce"], "challenge": q["code_challenge"], "redirect_uri": q["redirect_uri"], "scope": q.get("scope", "")}
            self._redirect(q["redirect_uri"] + sep + urllib.parse.urlencode({"code": code, "state": q["state"]}))
        elif u.path == "/userinfo":
            tok = self.headers.get("Authorization", "").removeprefix("Bearer ")
            who = state["tokens"].get(tok)
            if not who:
                return self._json(401, {"error": "invalid_token"})
            user = USERS[who]
            self._json(200, {"sub": user["sub"], **user.get("_userinfo", {})})
        else:
            self._json(404, {})

    def do_POST(self):
        if self.path != "/token":
            return self._json(404, {})
        form = {k: v[0] for k, v in urllib.parse.parse_qs(self.rfile.read(int(self.headers.get("Content-Length", 0))).decode()).items()}
        state["last"] = {"has_secret": "client_secret" in form, "client_id": form.get("client_id"), "grant_type": form.get("grant_type"), "redirect_uri": form.get("redirect_uri")}
        with lock:
            grant = state["codes"].pop(form.get("code", ""), None)
        if not grant or form.get("grant_type") != "authorization_code":
            return self._json(400, {"error": "invalid_grant"})
        if form.get("client_id") != CLIENT_ID or form.get("client_secret") != CLIENT_SECRET:
            return self._json(401, {"error": "invalid_client"})
        if form.get("redirect_uri") != grant["redirect_uri"]:
            return self._json(400, {"error": "invalid_grant", "error_description": "redirect_uri mismatch"})
        if b64(hashlib.sha256(form.get("code_verifier", "").encode()).digest()) != grant["challenge"]:
            return self._json(400, {"error": "invalid_grant", "error_description": "PKCE verification failed"})
        user = USERS[grant["user"]]
        access = secrets.token_urlsafe(16)
        state["tokens"][access] = grant["user"]
        out = {"access_token": access, "token_type": "Bearer", "expires_in": 300}
        if grant["fault"] != "noidtoken":
            out["id_token"] = id_token(user, grant["nonce"], grant["fault"])
        self._json(200, out)


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
