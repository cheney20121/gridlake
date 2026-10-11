"""直通游标：无筛选 / 搜索 / 排序时位置就是行号，不物化位置表

直通与物化两条路径对同一份数据必须给出完全一样的结果（取行、选区统计、导出）；删过行（行号有空洞）时自动退回物化；
数据写入后游标作废；有筛选 / 排序 / 搜索 / 数据范围限制时不走直通。
"""
from lib import *  # noqa: F403
for t in (A, Bo, R): call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
def import_ds(tok, name, csv, mode="create", pk=None, dataset_id=None):
    s, r = call("POST", "/ledger/datasets/import/preview", tok, raw=mp(name + ".csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
    assert s == 200, (s, r)
    p = r["data"]
    body = {"token": p["token"], "mode": mode, "name": name, "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": pk or [], "fileName": p["fileName"]}
    if dataset_id: body["datasetId"] = dataset_id
    s, r = call("POST", "/ledger/datasets/import/commit", tok, body=body)
    assert s == 200, (s, r)
    return r["data"]["datasetId"]

N = 1234
csv = "单号,店铺,金额\n" + "\n".join(f"O{i:05d},店{i % 7},{(i * 37) % 1000}.5" for i in range(N)) + "\n"
D = import_ds(A, "直通测试", csv, pk=["单号"])
spec = lambda **k: {"filters": [], "keyword": None, "sort": [], "rule": None, **k}
def cur(sp, tok=A):
    s, r = call("POST", f"/ledger/datasets/{D}/cursor", tok, body=sp); assert s == 200, (s, r); return r["data"]
def rows(c, off, n, tok=A):
    s, r = call("GET", f"/ledger/datasets/{D}/rows?cursor={c['cursorId']}&offset={off}&limit={n}", tok); assert s == 200, (s, r); return r["data"]

plain = cur(spec())
check("无筛选无排序走直通游标", plain["direct"] is True and plain["total"] == N, plain)
# 与物化路径对照：筛选里放一个恒真的区间（金额 ≥ 0），结果行集与顺序应完全一样
mat = cur(spec(filters=[{"op": "between", "column": "金额", "min": 0, "max": None}]))
check("带筛选走物化游标，行数相同", mat["direct"] is False and mat["total"] == N, mat)
same = all(rows(plain, off, 50)["rows"] == rows(mat, off, 50)["rows"] for off in (0, 1, 49, 600, N - 50, N - 7))
check("直通与物化：多个位置取出的行完全一致", same)
r = rows(plain, N - 3, 200)
check("末尾取行只返回剩下的 3 行", len(r["rows"]) == 3 and r["rows"][-1][0] == f"O{N-1:05d}", r["rows"][-1])
check("越界位置返回空", rows(plain, N + 100, 10)["rows"] == [])
check("起点为负按 0 处理", rows(plain, -5, 3)["rows"][0][0] == "O00000")
st = lambda c, a, b: call("POST", f"/ledger/datasets/{D}/stats", A, body={"cursorId": c["cursorId"], "from": a, "to": b, "columns": ["金额", "店铺"]})[1]["data"]
check("选区统计：直通与物化一致", st(plain, 10, 900) == st(mat, 10, 900), (st(plain, 10, 900), st(mat, 10, 900)))
check("选区统计：区间超过末尾", st(plain, 0, 10**9)["count"] == st(mat, 0, 10**9)["count"] == 2 * N, st(plain, 0, 10**9))
exp = lambda c, fmt: call("GET", f"/ledger/datasets/{D}/export?cursor={c['cursorId']}&format={fmt}", A, binary=True)
s1, b1 = exp(plain, "csv"); s2, b2 = exp(mat, "csv")
check("CSV 导出：直通与物化字节完全一致", s1 == s2 == 200 and b1 == b2 and b1.count(b"\n") == N + 1, (s1, s2, len(b1), len(b2)))
s3, x3 = exp(plain, "xlsx"); check("xlsx 导出可用", s3 == 200 and x3[:2] == b"PK", s3)

# 其他条件不走直通
check("按金额排序不走直通", cur(spec(sort=[{"column": "金额", "desc": True}]))["direct"] is False)
check("关键字搜索不走直通", cur(spec(keyword="店3"))["direct"] is False)
check("空白关键字等同没有搜索", cur(spec(keyword="  "))["direct"] is True)
check("再次打开命中缓存，仍是直通", (lambda c: c["cached"] and c["direct"])(cur(spec())))

# 数据写入后游标作废，新游标反映新数据；行号有空洞时退回物化
csv2 = "单号,店铺,金额\nO00005,店X,9999.5\nO00006,店Y,8888.5\n"   # 按主键更新 2 行（先删后插：留下行号空洞）
import_ds(A, "直通测试", csv2, mode="upsert", pk=["单号"], dataset_id=D)
expired = rows(plain, 0, 5)
check("写入后旧游标作废（expired）", expired.get("expired") is True, expired)
after = cur(spec())
mat2 = cur(spec(filters=[{"op": "between", "column": "金额", "min": 0, "max": None}]))
check("更新后（DuckDB 整理了行号）游标行数不变、两条路径行集一致", after["total"] == mat2["total"] == N and rows(after, 0, N)["rows"] == rows(mat2, 0, N)["rows"], after)
ra = rows(after, 0, N)["rows"]
check("更新后的行出现在末尾（先删后插），其余顺序不变", [r[0] for r in ra[-2:]] == ["O00005", "O00006"] and ra[0][0] == "O00000" and ra[5][0] == "O00007", [r[0] for r in ra[3:8]])

# 数据范围受限：不走直通（位置按各自可见的行排）
s, r = call("PUT", f"/ledger/data-scope/datasets/{D}", R, body={"scopeColumn": "店铺", "open": False}); check("绑定店铺列", s == 200, (s, r))
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Bob"], "mode": "shops", "shops": ["店0", "店1"]}); check("给 Bob 设置范围", s == 200, (s, r))
cb = cur(spec(), Bo)
check("受限用户不走直通，行数只含自己的店铺", cb["direct"] is False and 0 < cb["total"] < N, cb)
check("管理员 / 不受限用户仍走直通", cur(spec(), R).get("direct") in (True, False))
finish()
