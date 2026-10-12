"""对比两次浸泡测试结果（soak.py 的输出），找出明显变慢的请求类型。

用法：python3 compare.py <本次结果.json> [上次结果.json]
没有上次结果（第一次跑）时只打印本次。某类请求的 p50 或 p95 比上次慢 2 倍以上（且绝对值多出 200 ms 以上，避免毫秒级抖动误报）视为退步，退出码 1。
"""
import json
import os
import sys

cur = json.load(open(sys.argv[1]))
prev = json.load(open(sys.argv[2])) if len(sys.argv) > 2 and os.path.exists(sys.argv[2]) else None
print(f"{'请求':<18}{'次数':>8}{'p50 ms':>10}{'p95 ms':>10}{'上次 p50':>10}{'上次 p95':>10}")
regressions = []
for name, o in cur["ops"].items():
    p = (prev or {}).get("ops", {}).get(name, {})
    print(f"{name:<18}{o['count']:>8}{str(o['p50_ms']):>10}{str(o['p95_ms']):>10}{str(p.get('p50_ms', '-')):>10}{str(p.get('p95_ms', '-')):>10}")
    for key in ("p50_ms", "p95_ms"):
        a, b = o.get(key), p.get(key)
        if a is not None and b is not None and a > b * 2 and a - b > 200:
            regressions.append(f"{name} {key}: {b} → {a} ms")
print("内存（MB）：", cur.get("rss_mb"))
if prev is None:
    print("没有上次结果，作为基线。")
if regressions:
    print("\n性能退步：\n  " + "\n  ".join(regressions))
    sys.exit(1)
