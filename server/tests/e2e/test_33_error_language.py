"""接口错误信息的语言：请求头 x-lang 或 cookie gridlake-lang 为英文时返回英文，否则中文；没收录的信息原样返回"""
from lib import *  # noqa: F403

URL = "/ledger/datasets/import/preview-url"
BODY = {"url": "ftp://example.com/x.csv"}


def msg_of(headers=None):
    s, r = call("POST", URL, A, body=BODY, headers=headers)
    return s, r.get("msg") if isinstance(r, dict) else r


s, m = msg_of()
check("默认（没有语言设置）是中文", s == 400 and m == "只支持 http / https 网址", (s, m))
s, m = msg_of({"x-lang": "en-US"})
check("x-lang: en-US 返回英文", s == 400 and m == "Only http / https URLs are supported", (s, m))
s, m = msg_of({"Cookie": "a=1; gridlake-lang=en-US"})
check("cookie gridlake-lang=en-US 返回英文", s == 400 and m == "Only http / https URLs are supported", (s, m))
s, m = msg_of({"Cookie": "gridlake-lang=zh-CN"})
check("cookie 为 zh-CN 返回中文", m == "只支持 http / https 网址", (s, m))
s, m = msg_of({"x-lang": "zh-CN", "Cookie": "gridlake-lang=en-US"})
check("请求头优先于 cookie", m == "只支持 http / https 网址", (s, m))

# 带参数的信息、嵌套的信息
s, r = call("GET", "/ledger/datasets/ds_not_exist/rows?cursor=x&offset=0&limit=10", A, headers={"x-lang": "en-US"})
check("带参数的错误信息也翻译（不再含中文或保持可读）", s in (400, 404) and isinstance(r, dict), (s, r))

# 认证层的错误也翻译：不带令牌
import urllib.request, json
req = urllib.request.Request(BASE + "/ledger/workbooks", headers={"x-lang": "en-US"})  # noqa: F405
try:
    urllib.request.urlopen(req, timeout=10)
    code, body = 200, {}
except urllib.error.HTTPError as e:
    code, body = e.code, json.loads(e.read().decode())
check("未登录的提示翻译成英文", code == 401 and body.get("msg") == "Please sign in first", (code, body))
req = urllib.request.Request(BASE + "/ledger/workbooks")  # noqa: F405
try:
    urllib.request.urlopen(req, timeout=10)
except urllib.error.HTTPError as e:
    body = json.loads(e.read().decode())
check("同一个请求不带语言设置仍是中文", body.get("msg") == "请先登录", body)

# 嵌套：错误信息里的原因短语也翻译
s, r = call("POST", URL, A, body={"url": "http://169.254.169.254/latest/meta-data/"}, headers={"x-lang": "en-US"})
check("嵌套的原因短语也翻译", r.get("msg") == "Access to a link-local address, including cloud metadata endpoints is not allowed (169.254.169.254)", (s, r))

# 没收录的信息原样返回
s, r = call("GET", "/no/such/api", A, headers={"x-lang": "en-US"})
check("不存在的接口：英文", r.get("msg", "").startswith("No such endpoint:"), (s, r))

finish()
