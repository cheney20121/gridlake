# env: LEDGER_WEB_DIR=@WEB@
"""默认（没设 LEDGER_EMBED_ORIGINS）：所有页面包括分享页都只允许同源嵌入，防点击劫持"""
from lib import *  # noqa: F403
import urllib.request

ROOT = BASE.rsplit("/api", 1)[0]  # noqa: F405
for path in ["/", "/excel", "/login", "/share/gls_" + "x" * 43]:
    with urllib.request.urlopen(ROOT + path, timeout=15) as r:
        h = {k.lower(): v for k, v in r.getheaders()}
    check(f"默认只允许同源嵌入：{path.split('gls_')[0]}", h.get("content-security-policy") == "frame-ancestors 'self'" and h.get("x-frame-options") == "SAMEORIGIN" and h.get("x-content-type-options") == "nosniff", h)
    if path.startswith("/share/"):
        check("分享页不发 Referer", h.get("referrer-policy") == "no-referrer", h)
finish()
