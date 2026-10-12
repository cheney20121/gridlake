"""模拟登录网关：只实现 /current_user 与前端启动要用的 /webconfig。

令牌 → 用户：tok-alice / tok-bob / tok-carol 是普通用户，tok-root 是超级管理员；其他令牌一律 401。
GET /__shutdown 让它退出（测试身份服务不可用）。用法：python3 mock_gateway.py [端口，默认 19001]
"""
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

USERS = {
    "tok-alice": {"id": "u1", "username": "alice", "accountName": "Alice", "superAdmin": False, "tenantAdmin": False},
    "tok-bob": {"id": "u2", "username": "bob", "accountName": "Bob", "superAdmin": False, "tenantAdmin": False},
    "tok-carol": {"id": "u3", "username": "carol", "accountName": "Carol", "superAdmin": False, "tenantAdmin": False},
    "tok-root": {"id": "u0", "username": "root", "accountName": "Root", "superAdmin": True, "tenantAdmin": False},
}
WEBCONFIG = {"apiSign": False, "publicKey": "", "signSalt": "", "signVer": 1, "systemId": "s", "systemKey": "k", "tenantSwitchLogin": False,
             "customConfigs": {"captchaEnabled": False, "resetPwdOriginPasswordMode": False}}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _cors(self):
        self.send_header("Access-Control-Allow-Origin", self.headers.get("Origin", "*"))
        self.send_header("Access-Control-Allow-Headers", self.headers.get("Access-Control-Request-Headers", "*"))
        self.send_header("Access-Control-Allow-Methods", "GET,POST,PUT,DELETE,OPTIONS")

    def do_OPTIONS(self):
        self.send_response(204)
        self._cors()
        self.end_headers()

    def do_GET(self):
        path = self.path.split("?")[0]
        if path == "/__shutdown":
            self.send_response(200)
            self.end_headers()
            threading.Thread(target=self.server.shutdown, daemon=True).start()
            return
        if path.endswith("/webconfig"):
            body = {"code": 200, "data": WEBCONFIG, "msg": "success"}
        else:
            user = USERS.get(self.headers.get("x-user-token", ""))
            body = {"code": 200, "data": user, "msg": "success"} if user else {"code": 401, "data": None, "msg": "token invalid"}
        payload = json.dumps(body).encode()
        self.send_response(200)
        self._cors()
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 19001
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
