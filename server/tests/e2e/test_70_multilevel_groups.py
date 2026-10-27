"""多级分类汇总：先按店铺、再按渠道分组，每一级每组的行数与汇总，顺序与按两列依次排序的游标完全一致；
导出 Excel 时内层小计、外层小计、总计依次排列，分级是两级（左侧 1 / 2 / 3 按钮），折叠哪一级的哪一组都能带过去。
"""
import io
import re
import xml.etree.ElementTree as ET
import zipfile

from lib import *  # noqa: F403

NS = {"m": "http://schemas.openxmlformats.org/spreadsheetml/2006/main"}

for t in (A,):
    call("GET", "/ledger/me", t)
boundary = "XB"
def mp(name, csv):
    return (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: text/csv\r\n\r\n').encode() + csv.encode() + f"\r\n--{boundary}--\r\n".encode()
s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("两级订单.csv", "单号,店铺,渠道,金额\n1,北京店,直播,10\n2,上海店,直播,20\n3,北京店,短视频,30\n4,,直播,40\n5,上海店,短视频,50\n6,广州店,直播,60\n7,北京店,直播,70\n"), ctype=f"multipart/form-data; boundary={boundary}")
p = r["data"]
s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": "create", "name": "两级订单", "columns": [{"source": c["source"], "title": c["title"], "kind": {"金额": "money", "单号": "integer"}.get(c["title"], "text")} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
D = r["data"]["datasetId"]
spec = lambda **k: {"filters": [], "keyword": None, "sort": [], "rule": None, **k}
ITEMS = [{"column": "金额", "func": "sum"}, {"column": "单号", "func": "count"}]
LEVELS = [{"column": "店铺", "desc": False}, {"column": "渠道", "desc": False}]


def groups(levels=LEVELS, **k):
    return call("POST", f"/ledger/datasets/{D}/groups", A, body={**spec(**k), "levels": levels, "items": ITEMS})


def cursor(levels=LEVELS):
    s, r = call("POST", f"/ledger/datasets/{D}/cursor", A, body=spec(sort=levels))
    return r["data"]["cursorId"]


def runs(levels=LEVELS):
    """按游标的顺序数连续的段：每一级 [(取值, 行数)]"""
    c = cursor(levels)
    s, r = call("GET", f"/ledger/datasets/{D}/rows?cursor={c}&offset=0&limit=100", A)
    cols = [x["title"] for x in next(d for d in call("GET", "/ledger/datasets", A)[1]["data"] if d["id"] == D)["columns"]]
    idx = [cols.index(l["column"]) for l in levels]
    out = []
    for depth in range(1, len(levels) + 1):
        segs = []
        for row in r["data"]["rows"]:
            path = [row[i] for i in idx[:depth]]
            if segs and segs[-1][0] == path:
                segs[-1][1] += 1
            else:
                segs.append([path, 1])
        out.append([[p[-1], n] for p, n in segs])
    return out


s, r = groups()
lv = r["data"]["levels"]
check("两级：外层按店铺、内层按渠道，各组行数与汇总", s == 200 and len(lv) == 2
      and [(x["key"], x["count"], x["values"][0]) for x in lv[0]] == [("上海店", 2, 70.0), ("北京店", 3, 110.0), ("广州店", 1, 60.0), (None, 1, 40.0)]
      and [(x["key"], x["count"], x["values"][0]) for x in lv[1]] == [("直播", 1, 20.0), ("短视频", 1, 50.0), ("直播", 2, 80.0), ("短视频", 1, 30.0), ("直播", 1, 60.0), ("直播", 1, 40.0)], r)
check("groups 仍是最外层（兼容只有一级时的用法）", r["data"]["groups"] == lv[0], r["data"]["groups"])
check("每一级的顺序都与按两列排序的游标一致", [[[x["key"], x["count"]] for x in l] for l in lv] == runs(), (lv, runs()))
desc = [{"column": "店铺", "desc": True}, {"column": "渠道", "desc": False}]
s, r = groups(levels=desc)
check("外层降序、内层升序也一致（空值都排最后）", [[[x["key"], x["count"]] for x in l] for l in r["data"]["levels"]] == runs(desc), (r["data"]["levels"], runs(desc)))
s, r = groups(filters=[{"op": "in", "column": "渠道", "values": ["直播"]}])
check("带筛选", [(x["key"], x["count"]) for x in r["data"]["levels"][1]] == [("直播", 1), ("直播", 2), ("直播", 1), ("直播", 1)], r["data"]["levels"])
s, r = groups(levels=LEVELS + [{"column": "单号"}, {"column": "金额"}])
check("最多 3 级", s == 400 and "最多 3 级" in msg((s, r)), (s, r))
s, r = groups(levels=[{"column": "店铺"}, {"column": "店铺"}])
check("同一列不能分两级", s == 400 and "已经是一级分类汇总" in msg((s, r)), (s, r))
s, r = groups(levels=LEVELS + [{"column": "单号"}])
check("三级也行", s == 200 and [len(l) for l in r["data"]["levels"]] == [4, 6, 7], r)

# ── 导出 Excel：两级分级 ──
CUR = cursor()


def export(group):
    q = {"cursor": CUR, "columns": "单号,店铺,渠道,金额", "name": "导出", "format": "xlsx", "group": json.dumps(group, ensure_ascii=False)}
    return call("GET", f"/ledger/datasets/{D}/export?" + urllib.parse.urlencode(q), A, binary=True)


def sheet(data, n):
    z = zipfile.ZipFile(io.BytesIO(data))
    shared = []
    if "xl/sharedStrings.xml" in z.namelist():
        shared = ["".join(t.text or "" for t in si.iter(f"{{{NS['m']}}}t")) for si in ET.fromstring(z.read("xl/sharedStrings.xml")).findall("m:si", NS)]
    root = ET.fromstring(z.read(f"xl/worksheets/sheet{n}.xml"))
    fmt = root.find("m:sheetFormatPr", NS)
    out = {}
    for row in root.find("m:sheetData", NS).findall("m:row", NS):
        cells = {}
        for c in row.findall("m:c", NS):
            col = re.match(r"[A-Z]+", c.get("r")).group(0)
            f = c.find("m:f", NS)
            v = c.find("m:v", NS)
            val = v.text if v is not None else None
            if c.get("t") == "s" and val is not None:
                val = shared[int(val)]
            elif c.get("t") == "inlineStr":
                val = "".join(t.text or "" for t in c.iter(f"{{{NS['m']}}}t"))
            cells[col] = (val, f.text if f is not None else None)
        out[int(row.get("r"))] = (dict(row.attrib), cells)
    return out, (fmt.attrib if fmt is not None else {})


items = ITEMS + [{"column": "渠道", "func": "count"}]
s, data = export({"levels": LEVELS, "items": items, "collapsed": [[], [2]]})
check("导出成功", s == 200 and data[:2] == b"PK", (s, data[:200]))
rows, fmt = sheet(data, 2)
label = {r: (c.get("B", (None,))[0], c.get("C", (None,))[0]) for r, (_, c) in rows.items()}
check("内层小计在渠道列、外层小计在店铺列，最后总计", [label[r] for r in (3, 5, 6, 9, 12, 17, 18, 19)] == [(None, "直播 小计"), (None, "短视频 小计"), ("上海店 小计", "2"), (None, "直播 小计"), ("北京店 小计", "3"), (None, "直播 小计"), ("（空） 小计", "1"), ("总计", "7")] and max(rows) == 19, label)
check("内层小计只算这一组，外层小计跨过内层小计（SUBTOTAL 不重复算）", rows[9][1]["D"] == ("80", "SUBTOTAL(9,D7:D8)") and rows[12][1]["D"] == ("110", "SUBTOTAL(9,D7:D11)") and rows[19][1]["D"] == ("280", "SUBTOTAL(9,D2:D18)") and rows[12][1]["A"] == ("3", "SUBTOTAL(3,A7:A11)"), (rows[9][1], rows[12][1], rows[19][1]))
check("计数的列里有内层小计的名字时写算好的值（不让 SUBTOTAL 把名字也数进去）", rows[12][1]["C"] == ("3", None) and rows[19][1]["C"] == ("7", None), (rows[12][1]["C"], rows[19][1]["C"]))
lvl = {r: rows[r][0].get("outlineLevel") for r in rows}
check("分级：数据行 2 级、内层小计 1 级、外层小计和总计不分级", all(lvl[r] == "2" for r in (2, 4, 7, 8, 10, 13, 16)) and all(lvl[r] == "1" for r in (3, 5, 9, 11, 14, 17)) and all(lvl[r] is None for r in (1, 6, 12, 15, 18, 19)) and fmt.get("outlineLevelRow") == "2", (lvl, fmt))
check("折叠的内层组（北京店 / 直播）：数据行隐藏、小计行标记折叠", rows[7][0].get("hidden") == "1" and rows[8][0].get("hidden") == "1" and rows[9][0].get("collapsed") == "1" and "hidden" not in rows[9][0] and "hidden" not in rows[10][0], {r: rows[r][0] for r in (7, 8, 9, 10)})
try:
    import openpyxl
except ImportError:
    openpyxl = None
if openpyxl:
    ws = openpyxl.load_workbook(io.BytesIO(data))["数据"]
    check("openpyxl 能读：两级分级", ws.row_dimensions[7].outlineLevel == 2 and ws.row_dimensions[9].outlineLevel == 1 and ws.sheet_format.outlineLevelRow == 2, (ws.row_dimensions[7].outlineLevel, ws.sheet_format.outlineLevelRow))
summary, _ = sheet(data, 1)
note = [c.get("B", (None,))[0] for _, (_, c) in summary.items() if c.get("A", (None,))[0] == "分类汇总"]
check("「汇总」页签说明两级分组", note and "「店铺」（4 组） → 「渠道」（6 组）" in note[0] and "1 / 2 / 3" in note[0], note)

s, data = export({"levels": LEVELS, "items": ITEMS, "collapseLevel": 0})
rows, _ = sheet(data, 2)
hidden = sorted(r for r, (a, _) in rows.items() if a.get("hidden") == "1")
check("外层全部折叠：只剩各店铺小计和总计", hidden == [2, 3, 4, 5, 7, 8, 9, 10, 11, 13, 14, 16, 17] and all(rows[r][0].get("collapsed") == "1" for r in (6, 12, 15, 18)), hidden)
s, data = export({"levels": LEVELS, "items": ITEMS, "collapseLevel": 1})
rows, _ = sheet(data, 2)
hidden = sorted(r for r, (a, _) in rows.items() if a.get("hidden") == "1")
check("内层全部折叠：只剩内外层小计和总计", hidden == [2, 4, 7, 8, 10, 13, 16] and all(rows[r][0].get("collapsed") == "1" for r in (3, 5, 9, 11, 14, 17)), hidden)
finish()
