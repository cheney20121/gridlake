"""逐笔对账：银行流水 ↔ 企业账

随机造一份「带各种情形」的数据（正常到账有时差、金额差一分、超出日期窗口、一笔对多笔、只在一边的未达账项），
用 Python 独立推出每一轮应该勾出什么，与服务端对照；再检查：勾对结果不重复占用同一行、每个自动勾对组的约束成立、
剩下的行里不存在「本该勾上却没勾」的对子；数据重新导入后勾对保持，改了一行后只有那一组失效；
截至日的未达账项（含「对方日期晚于截至日」的）与公式下推（DSUMIFS）口径一致；权限与回收站。
"""
import random
from datetime import date, timedelta
from lib import *  # noqa: F403
for t in (A, Bo, C, R): call("GET", "/ledger/me", t)
time.sleep(1)
ids = {u["name"]: u["id"] for u in call("GET", "/ledger/users", A)[1]["data"]}

boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
def preview(name, csv, tok=A):
    s, r = call("POST", "/ledger/datasets/import/preview", tok, raw=mp(name + ".csv", csv), ctype=f"multipart/form-data; boundary={boundary}")
    assert s == 200, (s, r)
    return r["data"]
def import_ds(name, csv, tok=A):
    p = preview(name, csv, tok)
    s, r = call("POST", "/ledger/datasets/import/commit", tok, body={"token": p["token"], "mode": "create", "name": name, "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
    assert s == 200, (s, r)
    return r["data"]["datasetId"]
def replace_ds(ds, name, csv, tok=A):
    p = preview(name, csv, tok)
    s, r = call("POST", "/ledger/datasets/import/commit", tok, body={"token": p["token"], "mode": "replace", "datasetId": ds, "columns": [{"source": c["source"], "title": c["title"], "kind": c["kind"]} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
    assert s == 200, (s, r)

# ── 造数据（金额用「分」的整数，避免浮点）──
random.seed(14)
D0 = date(2026, 9, 1)
fmt = lambda d: d.isoformat()
bank, book = [], []          # 每行：[日期, 分, 参考号, 摘要]
truth = {}                   # 参考号 → 期望的勾对方式
def pair(ref, cents_b, cents_k, day, lag, kind):
    bank.append([D0 + timedelta(days=day + lag), cents_b, ref, f"银行-{ref}"])
    book.append([D0 + timedelta(days=day), cents_k, ref, f"凭证-{ref}"])
    truth[ref] = kind
n = 0
for i in range(300):                                       # 正常：金额一致、时差 0~3 天
    c = random.randint(1, 500000) * (1 if random.random() < 0.6 else -1)
    pair(f"R{i:04d}", c, c, random.randint(0, 27), random.randint(0, 3), "exact")
for i in range(6):                                         # 差一分：容差内
    c = random.randint(1000, 90000)
    pair(f"T{i:02d}", c + 1, c, random.randint(0, 27), 1, "tolerance")
for i in range(3):                                         # 差五分：容差外，留着
    c = random.randint(1000, 90000)
    pair(f"X{i:02d}", c + 5, c, random.randint(0, 27), 1, "none")
for i in range(4):                                         # 时差 9 天：超出日期窗口（只有开了「按键汇总」才会被第三轮勾上）
    c = random.randint(1000, 90000)
    pair(f"L{i:02d}", c, c, random.randint(0, 15), 9, "sum")
for i in range(8):                                         # 一笔对多笔：银行一笔 = 账面 2~4 笔之和
    parts = [random.randint(100, 30000) for _ in range(random.randint(2, 4))]
    day = random.randint(0, 25)
    bank.append([D0 + timedelta(days=day + 2), sum(parts), f"S{i:02d}", f"银行-S{i:02d}"])
    for j, p in enumerate(parts):
        book.append([D0 + timedelta(days=day + j % 2), p, f"S{i:02d}", f"凭证-S{i:02d}-{j}"])
    truth[f"S{i:02d}"] = "sum"
for i in range(25):                                        # 银行有、账面没有（手续费、利息）
    bank.append([D0 + timedelta(days=random.randint(0, 29)), -random.randint(100, 3000) if i % 3 else random.randint(100, 3000), f"B{i:03d}", f"银行-B{i:03d}"])
    truth[f"B{i:03d}"] = "bank-only"
for i in range(20):                                        # 账面有、银行没有（在途支票）
    book.append([D0 + timedelta(days=random.randint(0, 29)), -random.randint(5000, 80000), f"K{i:03d}", f"凭证-K{i:03d}"])
    truth[f"K{i:03d}"] = "book-only"
random.shuffle(bank); random.shuffle(book)
money = lambda c: f"{c / 100:.2f}"
bank_csv = "交易日期,收支金额,流水号,摘要\n" + "\n".join(f"{fmt(d)},{money(c)},{k},{m}" for d, c, k, m in bank) + "\n"
book_csv = "凭证日期,金额,参考号,摘要\n" + "\n".join(f"{fmt(d)},{money(c)},{k},{m}" for d, c, k, m in book) + "\n"
BANK = import_ds("银行流水", bank_csv)
BOOK = import_ds("企业账", book_csv)
check("两侧数据集导入", BANK and BOOK)

DEF = lambda **kw: {
    "left": {"dataset": "银行流水", "label": "银行", "amount": "收支金额", "date": "交易日期", "keys": ["流水号"], "memo": "摘要"},
    "right": {"dataset": "企业账", "label": "账面", "amount": "金额", "date": "凭证日期", "keys": ["参考号"], "memo": "摘要"},
    "tolerance": 0.02, "days": 5, "groupByKey": False, **kw}

# ── 1. 预览与创建 ──
s, r = call("POST", "/ledger/recons/preview", A, body={"def": DEF()})
check("预览成功", s == 200, (s, r))
pv = r["data"]
check("预览：两侧行数、金额合计、日期范围", pv["left"]["rows"] == len(bank) and pv["right"]["rows"] == len(book) and abs(pv["left"]["total"] - sum(x[1] for x in bank) / 100) < 0.005 and pv["left"]["minDate"] <= pv["left"]["maxDate"], pv["left"])
bad = DEF(); bad["left"]["dataset"] = "不存在"
s, r = call("POST", "/ledger/recons/preview", A, body={"def": bad}); check("数据集不存在有提示", s == 400 and "不存在" in msg((s, r)), (s, r))
bad = DEF(); bad["left"]["amount"] = "摘要"
s, r = call("POST", "/ledger/recons/preview", A, body={"def": bad}); check("金额列不是数值有提示", s == 400 and "不是数值列" in msg((s, r)), (s, r))
s, r = call("POST", "/ledger/recons", A, body={"name": "银行对账", "def": DEF()})
check("创建成功", s == 200 and r["data"].startswith("rc_"), (s, r))
RC = r["data"]
s, r = call("POST", "/ledger/recons", A, body={"name": "银行对账", "def": DEF()}); check("重名被拒", s == 400 and "已存在" in msg((s, r)), (s, r))
s, r = call("GET", "/ledger/datasets", A)
rc = next((d for d in r["data"] if d["id"] == RC), None)
check("结果数据集出现在清单里：类型 recon、行数 = 两侧之和、有「状态」「勾对完成日」列", rc and rc["kind"] == "recon" and rc["rowCount"] == len(bank) + len(book) and {"状态", "勾对完成日", "金额", "来源"} <= {c["title"] for c in rc["columns"]}, rc)

summary = lambda tok=A, q="": call("GET", f"/ledger/recons/{RC}/summary{q}", tok)
s, r = summary()
check("还没勾对：全部未勾对", s == 200 and r["data"]["left"]["matched"]["rows"] == 0 and r["data"]["groups"]["total"] == 0, r)

# ── 2. 试算不保存；第一次自动勾对（不含按键汇总）──
s, r = call("POST", f"/ledger/recons/{RC}/auto", A, body={"dryRun": True}); dry = r["data"]
check("试算：精确 300 + 容差 6", s == 200 and dry["dryRun"] and dry["exact"] == 300 and dry["tolerance"] == 6 and dry["sum"] == 0, dry)
check("试算后什么都没保存", summary()[1]["data"]["groups"]["total"] == 0)
s, r = call("POST", f"/ledger/recons/{RC}/auto", A, body={}); out = r["data"]
check("自动勾对：精确 300、容差 6、汇总 0", out["exact"] == 300 and out["tolerance"] == 6 and out["sum"] == 0 and out["groups"] == 306 and out["matchedRows"] == 612, out)

def all_groups():
    res, off = [], 0
    while True:
        s, r = call("GET", f"/ledger/recons/{RC}/groups?limit=200&offset={off}", A)
        assert s == 200, (s, r)
        res += r["data"]["groups"]
        off += 200
        if off >= r["data"]["total"]:
            return res
def open_rows(side, status="open"):
    res, off = [], 0
    while True:
        s, r = call("GET", f"/ledger/recons/{RC}/items?side={side}&status={status}&limit=500&offset={off}", A)
        assert s == 200, (s, r)
        res += r["data"]["rows"]
        off += 500
        if off >= r["data"]["total"]:
            return res

# 独立核对每个自动勾对组的约束
groups = all_groups()
bank_by_ref = {x[2]: x for x in bank}
ok_all = True; reused = False; seen = set()
for g in groups:
    L = [i for i in g["items"] if i["side"] == 0]; Rr = [i for i in g["items"] if i["side"] == 1]
    ok_all &= len(L) == 1 and len(Rr) == 1
    ok_all &= L[0]["corr"] == Rr[0]["corr"]
    diff = round(L[0]["amount"] - Rr[0]["amount"], 2)
    ok_all &= abs(diff) <= 0.02 + 1e-9 and abs(diff - g["diff"]) < 1e-9
    ok_all &= abs((date.fromisoformat(L[0]["date"]) - date.fromisoformat(Rr[0]["date"])).days) <= 5
    ok_all &= (g["method"] == "exact") == (diff == 0)
    for i in g["items"]:
        k = (i["side"], i["corr"], i["date"], i["amount"], i["memo"])
        reused |= k in seen
        seen.add(k)
check("每个自动勾对组：一对一、对账键相同、差额在容差内、日期在窗口内、方式与差额一致", ok_all and len(groups) == 306)
check("没有同一行被勾对两次", not reused)
by_ref = {g["items"][0]["corr"]: g["method"] for g in groups}
expect = {r: ("exact" if k == "exact" else "tolerance") for r, k in truth.items() if k in ("exact", "tolerance")}
check("勾对的恰好是应该勾上的那些对子（按参考号对照）", by_ref == expect, [(k, by_ref.get(k), expect.get(k)) for k in set(by_ref) ^ set(expect)][:5])

lo, ro = open_rows("left"), open_rows("right")
lref = sorted(x["corr"] for x in lo); rref = sorted(x["corr"] for x in ro)
want_l = sorted(r for r, k in truth.items() if k in ("none", "sum", "bank-only"))
want_r = sorted([r for r, k in truth.items() if k in ("none", "sum", "book-only")] + [r for r, k in truth.items() if k == "sum" and r.startswith("S") for _ in range(sum(1 for b in book if b[2] == r) - 1)])
check("未勾对（银行）= 容差外 3 + 超窗口 4 + 一对多 8 + 银行独有 25", lref == want_l, (len(lref), len(want_l)))
check("未勾对（账面）= 容差外 3 + 超窗口 4 + 一对多各笔 + 账面独有 20", rref == want_r, (len(rref), len(want_r)))

# 最大性：剩下的行里，同一参考号、金额相差在容差内、日期在窗口内的对子一个都不该有
rem_bad = []
for a in lo:
    for b in ro:
        if a["corr"] == b["corr"] and abs(a["amount"] - b["amount"]) <= 0.02 + 1e-9 and abs((date.fromisoformat(a["date"]) - date.fromisoformat(b["date"])).days) <= 5:
            rem_bad.append((a["corr"], a["amount"], b["amount"]))
check("没有漏勾的对子", not rem_bad, rem_bad[:3])

# ── 3. 打开「按键汇总」再跑：超窗口 4 对 + 一对多 8 组 ──
s, r = call("PUT", f"/ledger/recons/{RC}", A, body={"name": "银行对账", "def": DEF(groupByKey=True)}); check("改定义（不换数据集 / 金额列：无需清空勾对）", s == 200, (s, r))
s, r = call("POST", f"/ledger/recons/{RC}/auto", A, body={}); out = r["data"]
check("第二次：只剩汇总 12 组（超窗口 4 + 一对多 8）", out["exact"] == 0 and out["tolerance"] == 0 and out["sum"] == 12, out)
groups = all_groups()
sums = [g for g in groups if g["method"] == "sum"]
good = True
for g in sums:
    L = sum(i["amount"] for i in g["items"] if i["side"] == 0); Rr = sum(i["amount"] for i in g["items"] if i["side"] == 1)
    keys = {i["corr"] for i in g["items"]}
    good &= abs(L - Rr) <= 0.02 + 1e-9 and len(keys) == 1 and all(i["side"] in (0, 1) for i in g["items"]) and {0, 1} == {i["side"] for i in g["items"]}
check("汇总组：同一个键、两侧都有、合计相等", good and len(sums) == 12)
check("一对多的组有多行（最大的有 5 行）", max(len(g["items"]) for g in sums) >= 3, [len(g["items"]) for g in sums])
lo, ro = open_rows("left"), open_rows("right")
check("剩下：银行 3 + 25，账面 3 + 20", len(lo) == 28 and len(ro) == 23, (len(lo), len(ro)))

# ── 4. 汇总与截至日的未达账项（对照原始数据独立算）──
cent = lambda x: round(x * 100)
s, r = summary(); sm = r["data"]
check("汇总：笔数与金额", sm["left"]["all"]["rows"] == len(bank) and sm["right"]["all"]["rows"] == len(book) and abs(sm["left"]["all"]["total"] - sum(x[1] for x in bank) / 100) < 0.005 and sm["groups"]["total"] == 318 and sm["groups"]["broken"] == 0, sm)
want_open_l = [x for x in lo]; want_open_r = [x for x in ro]
check("汇总：银行未达 增加/减少", sm["left"]["openIn"]["rows"] == sum(1 for x in lo if x["amount"] > 0) and abs(sm["left"]["openOut"]["total"] - sum(x["amount"] for x in lo if x["amount"] < 0)) < 0.005, sm["left"])
# 截至 09-15：未勾对且日期 ≤ 09-15，加上已勾对但组内最晚日期 > 09-15 的
D = date(2026, 9, 15)
def outstanding(side_rows_open, tag):
    res = [(x["amount"]) for x in side_rows_open if x["date"] and date.fromisoformat(x["date"]) <= D]
    for g in groups:
        last = max(date.fromisoformat(i["date"]) for i in g["items"])
        if last > D:
            res += [i["amount"] for i in g["items"] if i["side"] == tag and date.fromisoformat(i["date"]) <= D]
    return res
for tag, name, rows_open in ((0, "left", lo), (1, "right", ro)):
    res = outstanding(rows_open, tag)
    s, r = summary(A, "?asOf=2026-09-15"); sd = r["data"][name]
    pos = [x for x in res if x > 0]; neg = [x for x in res if x < 0]
    check(f"截至 09-15 未达账项（{name}）：增加 {len(pos)} 笔 / 减少 {len(neg)} 笔，金额一致",
          sd["openIn"]["rows"] == len(pos) and sd["openOut"]["rows"] == len(neg) and abs(sd["openIn"]["total"] - sum(pos)) < 0.005 and abs(sd["openOut"]["total"] - sum(neg)) < 0.005, (sd, len(pos), len(neg), round(sum(pos), 2), round(sum(neg), 2)))
s, r = summary(A, "?asOf=2026/09/15"); check("截至日期格式不对有提示", s == 400 and "格式" in msg((s, r)), (s, r))

# 公式下推：状态 / 来源 / 日期 / 勾对完成日 的条件求和，与上面同口径
def dsum(criteria):
    s, r = call("POST", f"/ledger/datasets/{RC}/aggregate", A, body={"calls": [{"func": "sum", "column": "金额", "criteria": criteria}, {"func": "count", "criteria": criteria}]})
    assert s == 200, (s, r)
    return r["data"]["values"]
res = outstanding(lo, 0)
pos = round(sum(x for x in res if x > 0), 2)
v = dsum([{"column": "来源", "value": "银行"}, {"column": "日期", "value": "<2026-09-16"}, {"column": "金额", "value": ">0"}, {"column": "状态", "value": "未勾对"}])
v2 = dsum([{"column": "来源", "value": "银行"}, {"column": "日期", "value": "<2026-09-16"}, {"column": "金额", "value": ">0"}, {"column": "状态", "value": "已勾对"}, {"column": "勾对完成日", "value": ">=2026-09-16"}])
check("DSUMIFS：未勾对 + 已勾对但完成日晚于截至日 = 汇总里的银行未达（增加）", abs(v[0] + v2[0] - pos) < 0.005 and int(v[1] + v2[1]) == len([x for x in res if x > 0]), (v, v2, pos))
v = dsum([{"column": "状态", "value": "已勾对"}])
check("DSUMIFS：已勾对的行数 = 勾对组里的行数", int(v[1]) == sum(len(g["items"]) for g in groups), (v, sum(len(g["items"]) for g in groups)))
v = dsum([{"column": "状态", "value": "已勾对"}])
check("DSUMIFS：已勾对的金额两侧各自等于另一侧（不含差额）合计 = 差额合计", abs(v[0] - (sum(i["amount"] for g in groups for i in g["items"]))) < 0.005, v)

# 透视：来源 × 状态
s, r = call("POST", f"/ledger/datasets/{RC}/pivot", A, body={"rows": [{"column": "来源"}], "columns": [{"column": "状态"}], "values": [{"column": "金额", "agg": "count"}]})
cells = {x["keys"][0]: x["cells"] for x in r["data"]["rows"] if x["kind"] == "data"}
colkeys = [k[0] for k in r["data"]["columnKeys"]]
cnt = lambda who: dict(zip(colkeys, [c[0] for c in cells[who]]))
check("透视 来源×状态 的笔数", s == 200 and cnt("银行")["未勾对"] == len(lo) and cnt("银行")["已勾对"] == len(bank) - len(lo) and cnt("账面")["未勾对"] == len(ro) and cnt("账面")["已勾对"] == len(book) - len(ro), (colkeys, cells))

# ── 5. 手工勾对 ──
# 取一对未勾对的：银行独有的某笔 + 账面独有的某笔，金额不同 → 需要原因
bl = next(x for x in lo if x["corr"].startswith("B")); kb = next(x for x in ro if x["corr"].startswith("K"))
body = {"left": [bl["key"]], "right": [kb["key"]]}
s, r = call("POST", f"/ledger/recons/{RC}/match", A, body=body); check("差额超过容差：拒绝并说明", s == 400 and "超过了容差" in msg((s, r)), (s, r))
s, r = call("POST", f"/ledger/recons/{RC}/match", A, body={**body, "force": True}); check("强制勾对必须写原因", s == 400 and "原因" in msg((s, r)), (s, r))
s, r = call("POST", f"/ledger/recons/{RC}/match", A, body={"left": [bl["key"]], "right": []}); check("只有一边不能勾对", s == 400, (s, r))
s, r = call("POST", f"/ledger/recons/{RC}/match", A, body={**body, "force": True, "note": "手工核销：已与银行确认"}); m = r["data"]
check("写明原因后勾对成功，差额 = 左 - 右", s == 200 and abs(m["diff"] - round(bl["amount"] - kb["amount"], 2)) < 0.005 and m["gno"] > 0, (s, r))
s, r = call("POST", f"/ledger/recons/{RC}/match", A, body=body); check("同一行不能再勾对", s == 400 and "已经被勾对" in msg((s, r)), (s, r))
s, r = call("GET", f"/ledger/recons/{RC}/groups?method=manual", A); g = r["data"]["groups"][0]
check("勾对组列表：手工、带备注与勾对人", r["data"]["total"] == 1 and g["note"] == "手工核销：已与银行确认" and g["by"] == "Alice" and len(g["items"]) == 2, g)
s, r = call("GET", f"/ledger/recons/{RC}/groups?withDiff=true", A); check("只看有差额的组：容差配对 6 组（差 0.01）+ 手工 1 组", r["data"]["total"] == 7, r["data"]["total"])
# Bob（非所有者）也能勾对，但不能改定义 / 删除
bl2 = next(x for x in open_rows("left") if x["corr"].startswith("B")); kb2 = next(x for x in open_rows("right") if x["corr"].startswith("K"))
s, r = call("POST", f"/ledger/recons/{RC}/match", Bo, body={"left": [bl2["key"]], "right": [kb2["key"]], "force": True, "note": "Bob 核销"}); check("Bob 可以勾对", s == 200, (s, r))
s, r = call("GET", f"/ledger/recons/{RC}/groups?method=manual", A)
check("勾对人是 Bob", sorted(x["by"] for x in r["data"]["groups"]) == ["Alice", "Bob"], r["data"]["groups"])
s, r = call("PUT", f"/ledger/recons/{RC}", Bo, body={"name": "x", "def": DEF()}); check("Bob 不能改定义", s == 403, (s, r))
s, r = call("DELETE", f"/ledger/datasets/{RC}", Bo); check("Bob 不能删除对账", s == 403, (s, r))
# 取消
s, r = call("GET", f"/ledger/recons/{RC}/groups?method=manual", A)
gnos = [x["gno"] for x in r["data"]["groups"]]
s, r = call("POST", f"/ledger/recons/{RC}/unmatch", A, body={"groups": gnos}); check("取消两组手工勾对", s == 200 and r["data"]["groups"] == 2 and r["data"]["rows"] == 4, (s, r))
s, r = call("POST", f"/ledger/recons/{RC}/unmatch", A, body={}); check("取消要指定范围", s == 400, (s, r))
check("取消后回到未勾对", len(open_rows("left")) == 28 and len(open_rows("right")) == 23)

# ── 6. 重新导入后勾对保持；改了一行只有那一组失效 ──
sm0 = summary()[1]["data"]
before, before_rows = sm0["groups"]["total"], sm0["left"]["matched"]["rows"]
rows2 = bank[:]; random.shuffle(rows2)
replace_ds(BANK, "银行流水", "交易日期,收支金额,流水号,摘要\n" + "\n".join(f"{fmt(d)},{money(c)},{k},{m}" for d, c, k, m in rows2) + "\n")
sm = summary()[1]["data"]
check("银行流水整表重新导入（顺序打乱）：勾对组数与已勾对行数不变、没有失效", sm["groups"]["total"] == before and sm["groups"]["broken"] == 0 and sm["left"]["matched"]["rows"] == before_rows, (sm["groups"], sm["left"]["matched"], before_rows))
rows3 = [[d, c + (100000 if k == "R0001" else 0), k, m] for d, c, k, m in rows2]
replace_ds(BANK, "银行流水", "交易日期,收支金额,流水号,摘要\n" + "\n".join(f"{fmt(d)},{money(c)},{k},{m}" for d, c, k, m in rows3) + "\n")
sm = summary()[1]["data"]
check("改了 R0001 的金额：只有这一组失效", sm["groups"]["broken"] == 1, sm["groups"])
s, r = call("GET", f"/ledger/recons/{RC}/groups?broken=true", A); g = r["data"]["groups"][0]
check("失效的组可以查到，缺的那行标出来", r["data"]["total"] == 1 and any(i["missing"] for i in g["items"]), g)
s, r = call("GET", f"/ledger/recons/{RC}/items?side=right&status=open&q=R0001", A)
check("账面的 R0001 回到未勾对并带「已变动」标记", r["data"]["total"] == 1 and r["data"]["rows"][0]["stale"], r["data"])
s, r = call("POST", f"/ledger/datasets/{RC}/pivot", A, body={"rows": [{"column": "提示"}], "values": [{"column": "金额", "agg": "count"}]})
check("结果数据集里有「提示」列标出失效", s == 200 and any(x["keys"][0] and "变动" in str(x["keys"][0]) for x in r["data"]["rows"] if x["kind"] == "data"), r)
s, r = call("POST", f"/ledger/recons/{RC}/auto", A, body={}); out = r["data"]
check("再自动勾对：先清掉失效的 1 组；改过的金额差 1000 元，勾不上", out["clearedBroken"] == 1 and out["groups"] == 0, out)
sm = summary()[1]["data"]
check("清理后没有失效组，勾对组数 -1", sm["groups"]["broken"] == 0 and sm["groups"]["total"] == before - 1, sm["groups"])

# ── 7. 与数据集的生命周期 ──
s, r = call("DELETE", f"/ledger/datasets/{BANK}", A); check("被对账引用的数据集不能删", s == 400 and "银行对账" in msg((s, r)), (s, r))
s, r = call("POST", f"/ledger/datasets/{RC}/writeback", A, body={"mode": "append", "columns": ["金额"], "rows": [[1]]}); check("对账结果不能填报写入", s == 400 and "对账结果" in msg((s, r)), (s, r))
s, r = call("PUT", f"/ledger/recons/{RC}", A, body={"name": "银行对账", "def": {**DEF(), "left": {**DEF()["left"], "amount": "收支金额"}, "right": {**DEF()["right"], "dataset": "银行流水", "label": "账面"}}}); check("换成同一个数据集被拒", s == 400, (s, r))
d2 = DEF(); d2["right"]["sign"] = -1
s, r = call("PUT", f"/ledger/recons/{RC}", A, body={"name": "银行对账", "def": d2}); check("改金额方向会清空勾对：先要确认", s == 400 and "清空勾对" in msg((s, r)), (s, r))
s, r = call("PUT", f"/ledger/recons/{RC}", A, body={"name": "银行对账", "def": d2, "resetMatches": True}); check("确认后清空", s == 200 and summary()[1]["data"]["groups"]["total"] == 0, (s, r))
s, r = call("PUT", f"/ledger/recons/{RC}", A, body={"name": "银行对账（改名）", "def": DEF()}); check("再改回并改名（勾对为空，无需确认）", s == 200, (s, r))
s, r = call("GET", f"/ledger/recons/{RC}", A); check("取定义：规范成 id 与列 key", r["data"]["name"] == "银行对账（改名）" and r["data"]["def"]["left"]["dataset"] == BANK and r["data"]["def"]["left"]["amount"] == "收支金额", r)
s, r = call("DELETE", f"/ledger/datasets/{RC}", A); check("删除对账 → 回收站", s == 200, (s, r))
s, r = summary(); check("在回收站里，对账接口说不存在", s == 400, (s, r))
s, r = call("GET", "/ledger/datasets/trash", A); item = next((i for i in r["data"]["items"] if i["id"] == RC), None)
check("回收站里看得到，类型 recon，行数为空", item and item["kind"] == "recon" and item["rowCount"] is None and not item["blocked"], item)
s, r = call("POST", f"/ledger/datasets/{RC}/untrash", A); check("恢复", s == 200, (s, r))
s, r = call("POST", f"/ledger/recons/{RC}/auto", A, body={}); check("恢复后仍可勾对（R0001 的金额被改过，所以是 299 对）", s == 200 and r["data"]["exact"] == 299 and r["data"]["tolerance"] == 6, r)

# ── 8. 行级数据范围：受限用户不能做对账，结果数据集对他们不可见 ──
s, r = call("PUT", f"/ledger/data-scope/datasets/{BANK}", R, body={"scopeColumn": "摘要", "open": False}); check("管理员给银行流水绑定范围列", s == 200, (s, r))
s, r = call("PUT", f"/ledger/data-scope/datasets/{BOOK}", R, body={"open": True}); check("企业账设为所有人可见", s == 200, (s, r))
s, r = call("PUT", "/ledger/data-scope/rules", R, body={"subjectId": ids["Carol"], "mode": "shops", "shops": ["银行-R0001"]}); check("给 Carol 设范围", s == 200, (s, r))
s, r = call("GET", "/ledger/datasets", C); names = [d["name"] for d in r["data"]]
check("Carol 看不到对账结果数据集", "银行对账（改名）" not in names and "银行流水" in names, names)
s, r = call("GET", f"/ledger/recons/{RC}/summary", C); check("Carol 调对账接口被拒", s == 400, (s, r))
s, r = call("POST", "/ledger/recons/preview", C, body={"def": DEF()}); check("Carol 预览对账被拒（只看得到一部分数据）", s == 403 and "行级数据范围" in msg((s, r)), (s, r))
s, r = call("POST", f"/ledger/recons/{RC}/auto", C, body={}); check("Carol 不能自动勾对", s == 400, (s, r))
s, r = call("GET", f"/ledger/recons/{RC}/summary", A); check("Alice 不受影响", s == 200, (s, r))

finish()
