# env: LEDGER_WEB_DIR=@WEB@ LEDGER_EMBED_ORIGINS=https://a.example.com,https://*.b.example.com
"""分享页可嵌入：LEDGER_EMBED_ORIGINS 只放开 /share/…，其他页面仍然只允许同源嵌入；分享页不通过 Referer 泄露令牌"""
from lib import *  # noqa: F403
import urllib.error
import urllib.request

ROOT = BASE.rsplit("/api", 1)[0]  # noqa: F405


def page(path):
    try:
        with urllib.request.urlopen(ROOT + path, timeout=15) as r:
            return r.status, {k.lower(): v for k, v in r.getheaders()}, r.read()
    except urllib.error.HTTPError as e:
        return e.code, {k.lower(): v for k, v in e.headers.items()}, b""


s, h, body = page("/share/gls_" + "x" * 43)
check("分享页（单页应用回退到 index.html）", s == 200 and b"<title>e2e</title>" in body, (s, body[:60]))
check("只允许配置的网站嵌入", h.get("content-security-policy") == "frame-ancestors https://a.example.com https://*.b.example.com", h)
check("开放嵌入时不再带 X-Frame-Options（它表达不了「这几个网站」）", "x-frame-options" not in h, h)
check("分享页不发 Referer：令牌不会泄露给嵌入页或别的网站", h.get("referrer-policy") == "no-referrer", h)
check("nosniff", h.get("x-content-type-options") == "nosniff", h)

for path in ["/", "/excel", "/login", "/excel/home", "/sharing"]:
    s, h, _ = page(path)
    check(f"其他页面不开放：{path}", s == 200 and h.get("content-security-policy") == "frame-ancestors 'self'" and h.get("x-frame-options") == "SAMEORIGIN" and "referrer-policy" not in h, (s, h))

s, h, _ = page("/api/health")
check("接口响应不是页面，不加页面头", s == 200 and "content-security-policy" not in h, h)
finish()
