# env: LEDGER_ADMIN_TOKEN=ops-secret-1
"""运行指标（Prometheus 文本格式）：访问控制、请求计数、耗时直方图、运行状态"""
from lib import *  # noqa: F403
import re

OPS = {"x-admin-token": "ops-secret-1"}


def metrics(headers=None):
    s, r = call("GET", "/admin/metrics", headers=headers, binary=True)
    return s, (r.decode() if isinstance(r, bytes) else str(r))


def value(text, name):
    m = re.search(r"^" + re.escape(name) + r" ([0-9.eE+-]+)$", text, re.M)
    return float(m.group(1)) if m else None


s, _ = metrics()
check("没有运维令牌：拒绝", s == 403, s)
s, _ = metrics({"x-admin-token": "wrong"})
check("令牌不对：拒绝", s == 403, s)
s, text = metrics(OPS)
check("x-admin-token 可以访问", s == 200 and "# TYPE gridlake_http_requests_total counter" in text, (s, text[:200]))
s, text2 = metrics({"Authorization": "Bearer ops-secret-1"})
check("Authorization: Bearer 也可以（Prometheus 抓取用）", s == 200 and "gridlake_build_info" in text2, (s, text2[:200]))

before = value(text2, 'gridlake_http_requests_total{code="2xx"}') or 0
for _ in range(3):
    call("GET", "/health")
call("GET", "/ledger/workbooks", A)   # 网关里的有效令牌
call("GET", "/ledger/workbooks")      # 没带令牌：401
s, t = metrics(OPS)
after2xx = value(t, 'gridlake_http_requests_total{code="2xx"}')
after4xx = value(t, 'gridlake_http_requests_total{code="4xx"}')
check("2xx 请求数增加", after2xx is not None and after2xx >= before + 4, (before, after2xx))
check("4xx 请求数增加（未登录的 401）", after4xx is not None and after4xx >= 1, after4xx)
count = value(t, "gridlake_http_request_duration_seconds_count")
inf = value(t, 'gridlake_http_request_duration_seconds_bucket{le="+Inf"}')
check("直方图的 +Inf 桶等于总数", count is not None and count == inf and count >= 5, (count, inf))
check("运行状态指标都在", all(value(t, n) is not None for n in ["gridlake_uptime_seconds", "gridlake_active_queries", "gridlake_heavy_queries_limit"]) and 'gridlake_heavy_queries{state="waiting"}' in t, t[:300])
check("数据文件大小大于 0", (value(t, 'gridlake_db_size_bytes{file="main"}') or 0) > 0, t)
check("版本号", 'gridlake_build_info{version="' in t, t[:300])
finish()
