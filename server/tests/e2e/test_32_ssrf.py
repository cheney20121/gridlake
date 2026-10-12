"""服务端主动访问用户填写的网址时的地址限制（SSRF）：本机、云元数据地址及各种写法都要拒绝"""
from lib import *  # noqa: F403

BAD = [
    ("回环地址", "http://127.0.0.1:8787/api/health"),
    ("localhost", "http://localhost/x.csv"),
    ("IPv6 回环", "http://[::1]/x.csv"),
    ("IPv4 映射的 IPv6", "http://[::ffff:127.0.0.1]/x.csv"),
    ("AWS / GCP 元数据", "http://169.254.169.254/latest/meta-data/"),
    ("阿里云元数据", "http://100.100.100.200/latest/meta-data/"),
    ("十进制写法的 127.0.0.1", "http://2130706433/x.csv"),
    ("十六进制写法", "http://0x7f.0.0.1/x.csv"),
    ("0.0.0.0", "http://0.0.0.0:8787/x.csv"),
]
for label, url in BAD:
    s, r = call("POST", "/ledger/datasets/import/preview-url", A, body={"url": url})
    check(f"网址导入拒绝：{label}", s == 400 and "不能访问" in str(r.get("msg", "")), (s, r))

s, r = call("POST", "/ledger/datasets/import/preview-url", A, body={"url": "ftp://example.com/x.csv"})
check("网址导入拒绝：非 http / https", s == 400 and "http" in str(r.get("msg", "")), (s, r))
s, r = call("POST", "/ledger/datasets/import/preview-url", A, body={"url": "file:///etc/passwd"})
check("网址导入拒绝：file://", s == 400, (s, r))

finish()
