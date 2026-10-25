"""分类汇总导出 Excel：「数据」页签每组后面一行小计（SUBTOTAL 公式，带算好的值）、最后一行总计；
每组的数据行设成 Excel 分级（可折叠），在界面上折叠的组导出后也是折叠的；「汇总」页签说明分组情况。
不带分类汇总时照旧（没有分级）。用标准库解析 xlsx（不依赖第三方包）。
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
s, r = call("POST", "/ledger/datasets/import/preview", A, raw=mp("导出订单.csv", "单号,店铺,金额\n1,北京店,10\n2,上海店,20\n3,北京店,30\n4,,40\n5,上海店,50\n6,广州店,60\n7,北京店,70\n"), ctype=f"multipart/form-data; boundary={boundary}")
p = r["data"]
s, r = call("POST", "/ledger/datasets/import/commit", A, body={"token": p["token"], "mode": "create", "name": "导出订单", "columns": [{"source": c["source"], "title": c["title"], "kind": {"金额": "money", "单号": "integer"}.get(c["title"], "text")} for c in p["columns"]], "primaryKey": [], "fileName": p["fileName"]})
D = r["data"]["datasetId"]

s, r = call("POST", f"/ledger/datasets/{D}/cursor", A, body={"filters": [], "sort": [{"column": "店铺", "desc": False}]})
CUR = r["data"]["cursorId"]
ITEMS = [{"column": "金额", "func": "sum"}, {"column": "单号", "func": "count"}]


def export(group=None):
    q = {"cursor": CUR, "columns": "单号,店铺,金额", "name": "导出", "format": "xlsx"}
    if group is not None:
        q["group"] = json.dumps(group, ensure_ascii=False)
    return call("GET", f"/ledger/datasets/{D}/export?" + urllib.parse.urlencode(q), A, binary=True)


def sheet(data, n):
    """第 n 个页签：[(行号, 行属性, {列: (值, 公式)})]"""
    z = zipfile.ZipFile(io.BytesIO(data))
    shared = []
    if "xl/sharedStrings.xml" in z.namelist():
        shared = ["".join(t.text or "" for t in si.iter(f"{{{NS['m']}}}t")) for si in ET.fromstring(z.read("xl/sharedStrings.xml")).findall("m:si", NS)]
    root = ET.fromstring(z.read(f"xl/worksheets/sheet{n}.xml"))
    out = []
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
        out.append((int(row.get("r")), dict(row.attrib), cells))
    return out


s, data = export({"column": "店铺", "desc": False, "items": ITEMS, "collapsed": [1]})
check("导出成功", s == 200 and data[:2] == b"PK", (s, data[:200]))
rows = sheet(data, 2)
labels = [(r, cells.get("B", (None,))[0]) for r, _, cells in rows]
check("每组后面一行小计、最后一行总计（空值组排最后）", [x[1] for x in labels] == ["店铺", "上海店", "上海店", "上海店 小计", "北京店", "北京店", "北京店", "北京店 小计", "广州店", "广州店 小计", None, "（空） 小计", "总计"], labels)
by = {r: (attrs, cells) for r, attrs, cells in rows}
check("小计是 SUBTOTAL 公式，带算好的值", by[4][1]["C"] == ("70", "SUBTOTAL(9,C2:C3)") and by[4][1]["A"] == ("2", "SUBTOTAL(3,A2:A3)"), by[4][1])
check("总计按全部行（SUBTOTAL 会跳过里面的小计）", by[13][1]["C"] == ("280", "SUBTOTAL(9,C2:C12)") and by[13][1]["A"][0] == "7", by[13][1])
check("每组的数据行是一级分级", all(by[r][0].get("outlineLevel") == "1" for r in (2, 3, 5, 6, 7, 9, 11)) and "outlineLevel" not in by[4][0] and "outlineLevel" not in by[13][0], {r: by[r][0] for r in by})
check("界面上折叠的组（第 2 组北京店）导出后也是折叠的", all(by[r][0].get("hidden") == "1" for r in (5, 6, 7)) and by[8][0].get("collapsed") == "1" and "hidden" not in by[2][0], {r: by[r][0] for r in (2, 5, 8)})
try:
    import openpyxl  # 有就用它再读一遍：改写后的文件对真正的 xlsx 读取器也是好的
except ImportError:
    openpyxl = None
if openpyxl:
    wb = openpyxl.load_workbook(io.BytesIO(data))
    ws = wb["数据"]
    check("openpyxl 能读：分级、隐藏、折叠都在", ws.row_dimensions[5].outlineLevel == 1 and ws.row_dimensions[5].hidden and ws.row_dimensions[8].collapsed and ws.sheet_format.outlineLevelRow == 1 and ws["B13"].value == "总计", (ws.row_dimensions[5].outlineLevel, ws.sheet_format.outlineLevelRow))
summary = sheet(data, 1)
note = [cells.get("B", (None,))[0] for _, _, cells in summary if cells.get("A", (None,))[0] == "分类汇总"]
check("「汇总」页签说明分组情况", note and "按「店铺」分为 4 组" in note[0], note)
sumf = [cells for _, _, cells in summary if cells.get("A", (None,))[0] == "金额"]
check("「汇总」页签的合计覆盖到总计行", sumf and sumf[0]["B"][1] == "SUBTOTAL(109,'数据'!C2:C13)", sumf)

s, data = export({"column": "店铺", "desc": False, "items": ITEMS, "allCollapsed": True})
rows = {r: attrs for r, attrs, _ in sheet(data, 2)}
check("全部折叠：所有数据行都隐藏，只剩小计和总计", all(rows[r].get("hidden") == "1" for r in (2, 3, 5, 6, 7, 9, 11)) and "hidden" not in rows[4] and "hidden" not in rows[13] and all(rows[r].get("collapsed") == "1" for r in (4, 8, 10, 12)), rows)

s, data = export()
rows = sheet(data, 2)
check("不带分类汇总时照旧：8 行、没有分级", len(rows) == 8 and not any("outlineLevel" in a for _, a, _ in rows), [a for _, a, _ in rows])
s, r = export({"column": "没有这列", "items": []})
check("分组列不存在：报错", s >= 400, (s, r[:200] if isinstance(r, bytes) else r))
s, r = call("GET", f"/ledger/datasets/{D}/export?" + urllib.parse.urlencode({"cursor": CUR, "format": "xlsx", "group": "{坏"}), A)
check("参数不是 JSON：说明", s == 400 and "分类汇总参数不正确" in msg((s, r)), (s, r))
finish()
