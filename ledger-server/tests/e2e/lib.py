"""端到端回归测试的公共工具（只用标准库）。

约定：每个 test_*.py 对应一个全新的服务与临时数据库（见 run.sh），所以测试可以假定库是空的、
也不用清理；用 check(名称, 条件) 记录结果，最后 finish() 汇总并以退出码反映成败。
"""
import gzip  # noqa: F401  测试脚本里直接用
import json
import os
import sys
import time  # noqa: F401
import urllib.error
import urllib.parse  # noqa: F401
import urllib.request

BASE = os.environ.get("LEDGER_E2E_BASE", "http://127.0.0.1:18787/api")
GATEWAY = os.environ.get("LEDGER_E2E_GATEWAY", "http://127.0.0.1:19001")

# mock_gateway.py 里的四个用户：Alice / Bob / Carol 是普通用户，Root 是管理员
A, Bo, C, R = "tok-alice", "tok-bob", "tok-carol", "tok-root"

_fails = 0
_total = 0


def call(method, path, tok=None, body=None, raw=None, ctype=None, headers=None, binary=False):
    """调接口，返回 (HTTP 状态码, 解析后的 JSON 或原始字节)。HTTP 错误也正常返回，不抛异常。"""
    h = dict(headers or {})
    if tok:
        h["x-user-token"] = tok
    data = None
    if raw is not None:
        data = raw
        h["Content-Type"] = ctype or "application/octet-stream"
    elif body is not None:
        data = json.dumps(body).encode()
        h["Content-Type"] = "application/json"
    req = urllib.request.Request(BASE + path, data=data, method=method, headers=h)
    try:
        with urllib.request.urlopen(req, timeout=float(os.environ.get("LEDGER_E2E_TIMEOUT", "30"))) as resp:
            text = resp.read()
            if binary:
                return resp.status, text
            try:
                return resp.status, json.loads(text)
            except Exception:
                return resp.status, text
    except urllib.error.HTTPError as e:
        text = e.read()
        try:
            return e.code, json.loads(text)
        except Exception:
            return e.code, text


def msg(r):
    """接口返回里给用户看的说明（r 为 call 的返回值）。"""
    return r[1].get("msg") if isinstance(r[1], dict) else str(r[1])


def check(name, cond, extra=""):
    global _fails, _total
    _total += 1
    print(("PASS " if cond else "FAIL ") + name + ((" -> " + str(extra)[:400]) if not cond else ""))
    if not cond:
        _fails += 1


def stop_gateway():
    """让模拟网关退出（测试「身份服务不可用」用）。"""
    try:
        urllib.request.urlopen(GATEWAY + "/__shutdown", timeout=5).read()
    except Exception:
        pass
    time.sleep(0.5)


def finish():
    print(f"\n{_total - _fails}/{_total} 通过" + (f"，{_fails} 项失败" if _fails else ""))
    sys.exit(1 if _fails else 0)
