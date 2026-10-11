#!/usr/bin/env python3
"""生成内置模板（templates/*.json）：python3 templates/gen.py

模板里的数据集用占位 ID（如 $orders）引用，使用模板时换成使用者对应的数据集；
引用内置数据集「资金流水」（ledger_detail）的直接写真实 ID，使用时自动对应。
工作簿结构与前端保存的 Univer 快照一致：浮动部件放在 SHEET_DRAWING_PLUGIN 资源里，透视表定义在工作表 custom.pivot。
"""

import json
import os

UNIT = 'webexcel-template'
COL_W, ROW_H, HEAD_W, HEAD_H = 88, 24, 46, 20
OUT = os.path.dirname(os.path.abspath(__file__))

STYLES = {
    'title': {'ff': 'Microsoft YaHei', 'fs': 14, 'bl': 1},
    'note': {'ff': 'Microsoft YaHei', 'fs': 10, 'cl': {'rgb': '#6B7280'}},
    'header': {'ff': 'Microsoft YaHei', 'fs': 10, 'bl': 1, 'bg': {'rgb': '#DDEBF7'}, 'ht': 2},
    'input': {'ff': 'Microsoft YaHei', 'fs': 10, 'bg': {'rgb': '#FFF7E6'}, 'bd': {'b': {'s': 1, 'cl': {'rgb': '#F59E0B'}}}},
    'label': {'ff': 'Microsoft YaHei', 'fs': 10},
    'helper': {'ff': 'Microsoft YaHei', 'fs': 9, 'cl': {'rgb': '#9CA3AF'}},
    'money': {'ff': 'Microsoft YaHei', 'fs': 10, 'ht': 3, 'n': {'pattern': '#,##0.00;-#,##0.00;"-"'}},
    'ratio': {'ff': 'Microsoft YaHei', 'fs': 10, 'ht': 3, 'n': {'pattern': '0.00'}},
    'pct': {'ff': 'Microsoft YaHei', 'fs': 10, 'ht': 3, 'n': {'pattern': '0.0%'}},
    'qty': {'ff': 'Microsoft YaHei', 'fs': 10, 'ht': 3},
    'totalQty': {'ff': 'Microsoft YaHei', 'fs': 10, 'bl': 1, 'ht': 3, 'bd': {'t': {'s': 1, 'cl': {'rgb': '#4472C4'}}}},
    'totalLabel': {'ff': 'Microsoft YaHei', 'fs': 10, 'bl': 1, 'bd': {'t': {'s': 1, 'cl': {'rgb': '#4472C4'}}}},
    'totalMoney': {'ff': 'Microsoft YaHei', 'fs': 10, 'bl': 1, 'ht': 3, 'n': {'pattern': '#,##0.00;-#,##0.00;"-"'}, 'bd': {'t': {'s': 1, 'cl': {'rgb': '#4472C4'}}}},
    'totalRatio': {'ff': 'Microsoft YaHei', 'fs': 10, 'bl': 1, 'ht': 3, 'n': {'pattern': '0.00'}, 'bd': {'t': {'s': 1, 'cl': {'rgb': '#4472C4'}}}},
    'totalPct': {'ff': 'Microsoft YaHei', 'fs': 10, 'bl': 1, 'ht': 3, 'n': {'pattern': '0.0%'}, 'bd': {'t': {'s': 1, 'cl': {'rgb': '#4472C4'}}}},
}


class Book:
    def __init__(self, name):
        self.name = name
        self.sheets = {}
        self.order = []
        self.drawings = {}
        self.seq = 0

    def sheet(self, sid, name, gridlines=True, custom=None, col_widths=None):
        s = {
            'id': sid, 'name': name, 'tabColor': '', 'hidden': 0, 'rowCount': 1000, 'columnCount': 26,
            'zoomRatio': 1, 'scrollTop': 0, 'scrollLeft': 0, 'defaultColumnWidth': COL_W, 'defaultRowHeight': ROW_H,
            'showGridlines': 1 if gridlines else 0, 'rightToLeft': 0,
            'freeze': {'xSplit': 0, 'ySplit': 0, 'startRow': -1, 'startColumn': -1},
            'rowHeader': {'width': HEAD_W, 'hidden': 0}, 'columnHeader': {'height': HEAD_H, 'hidden': 0},
            'mergeData': [], 'rowData': {}, 'columnData': {}, 'cellData': {},
        }
        if custom:
            s['custom'] = custom
        for c, w in (col_widths or {}).items():
            s['columnData'][str(c)] = {'w': w, 'hd': 0}
        self.sheets[sid] = s
        self.order.append(sid)
        return s

    def cell(self, sid, r, c, v=None, f=None, s=None):
        cell = {}
        if v is not None:
            cell['v'] = v
        if f is not None:
            cell['f'] = f
        if s is not None:
            cell['s'] = s
        self.sheets[sid]['cellData'].setdefault(str(r), {})[str(c)] = cell

    def part(self, sid, key, spec, left, top, width, height):
        """浮动部件：left / top 为画布坐标（含行列表头），与前端 insertFloat 一致"""
        self.seq += 1
        did = f'{sid}-p{self.seq}'
        x, y = left - HEAD_W, top - HEAD_H

        def anchor(px, py):
            return {'column': px // COL_W, 'columnOffset': px % COL_W, 'row': py // ROW_H, 'rowOffset': py % ROW_H}

        st = {'from': anchor(x, y), 'to': anchor(x + width, y + height)}
        self.drawings.setdefault(sid, {'data': {}, 'order': []})
        self.drawings[sid]['data'][did] = {
            'unitId': UNIT, 'subUnitId': sid, 'drawingId': did, 'drawingType': 8, 'componentKey': key,
            'sheetTransform': st, 'axisAlignSheetTransform': st,
            'transform': {'flipY': False, 'flipX': False, 'angle': 0, 'skewX': 0, 'skewY': 0, 'left': left, 'top': top, 'width': width, 'height': height},
            'data': {**spec, 'id': f'{key.split("-")[1]}_{did}'},
            'allowTransform': True,
        }
        self.drawings[sid]['order'].append(did)

    def json(self):
        return {
            'id': UNIT, 'name': self.name, 'appVersion': '1.0.2', 'locale': 'zhCN', 'styles': STYLES,
            'sheetOrder': self.order, 'sheets': self.sheets,
            'resources': [{'name': 'SHEET_DRAWING_PLUGIN', 'data': json.dumps(self.drawings, ensure_ascii=False)}],
        }


def slicer(title, ds, column, mode='values', preset=None):
    return {'v': 1, 'kind': 'slicer', 'title': title, 'datasetId': ds, 'column': column, 'mode': mode, 'values': [],
            'range': {'preset': preset} if preset else None}


def kpi(title, ds, column, agg, date_col, compare, up_is_good=True, filters=None, sparkline=True):
    return {'v': 1, 'kind': 'kpi', 'title': title, 'datasetId': ds, 'value': {'column': column, 'agg': agg},
            'dateColumn': date_col, 'granularity': 'month', 'compare': compare, 'upIsGood': up_is_good,
            'sparkline': sparkline, 'filters': filters or []}


def table(title, ds, row, values, top=10, sort='value', granularity=None, filters=None):
    return {'v': 1, 'kind': 'table', 'title': title, 'datasetId': ds, 'row': {'column': row, 'granularity': granularity},
            'values': values, 'top': top, 'sort': sort, 'filters': filters or []}


def chart(title, kind, ds, x, values, granularity=None, split=None, filters=None, sort='x', top=None, stacked=False):
    return {'v': 1, 'type': kind, 'title': title,
            'source': {'kind': 'dataset', 'datasetId': ds, 'x': {'column': x, 'granularity': granularity}, 'values': values,
                       'splitBy': split, 'filters': filters or [], 'sort': sort, 'top': top},
            'stacked': stacked, 'legend': 'top', 'labels': 'none', 'unit': 'auto'}


def pivot_sheet(book, sid, name, ds, ds_name, request, hint):
    book.sheet(sid, name, custom={'pivot': {'version': 1, 'datasetId': ds, 'datasetName': ds_name, 'request': {
        'filters': [], 'sort': {'by': 'key', 'valueIndex': 0, 'desc': False}, 'top': None, 'subtotals': True, 'grandTotals': True, **request}}})
    book.cell(sid, 0, 0, f'数据透视表 · {hint}（打开此页签时按最新数据计算）', s='note')


def notes(book, sid, title, lines):
    book.sheet(sid, '使用说明', col_widths={0: 760})
    book.cell(sid, 0, 0, title, s='title')
    for i, line in enumerate(lines):
        book.cell(sid, i + 2, 0, line, s='label' if not line.startswith('·') else 'note')


def write(fname, meta, book):
    data = {**meta, 'workbook': book.json()}
    with open(os.path.join(OUT, fname), 'w', encoding='utf-8') as f:
        json.dump(data, f, ensure_ascii=False, indent=1)
    print('wrote', fname)


# ── 1. 月度经营分析（订单数据）──

ORDERS = '$orders'
orders_ds = {
    'key': ORDERS, 'name': '订单', 'description': '订单明细（每行一个订单或子订单），可来自导入或数据连接',
    'columns': [
        {'title': '店铺', 'kind': 'text', 'description': '店铺 / 渠道名称'},
        {'title': '下单时间', 'kind': 'datetime', 'description': '下单或支付时间'},
        {'title': '订单金额', 'kind': 'money', 'description': '成交金额（实付）'},
    ],
}

b = Book('月度经营分析')
b.sheet('dash', '经营看板', gridlines=False)
b.part('dash', 'webexcel-slicer', slicer('店铺', ORDERS, '店铺'), 58, 32, 200, 300)
b.part('dash', 'webexcel-slicer', slicer('下单时间', ORDERS, '下单时间', 'date', 'this_year'), 58, 344, 200, 170)
b.part('dash', 'webexcel-kpi', kpi('销售额', ORDERS, '订单金额', 'sum', '下单时间', 'yoy'), 270, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('订单数', ORDERS, None, 'count', '下单时间', 'mom'), 532, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('客单价', ORDERS, '订单金额', 'avg', '下单时间', 'mom'), 794, 32, 250, 136)
b.part('dash', 'webexcel-chart', chart('各店铺月度销售额', 'line', ORDERS, '下单时间', [{'column': '订单金额', 'agg': 'sum'}], 'month', split='店铺'), 270, 180, 520, 330)
b.part('dash', 'webexcel-table', table('店铺销售排行', ORDERS, '店铺', [{'column': '订单金额', 'agg': 'sum'}, {'column': '订单金额', 'agg': 'sum', 'show': 'pct_total'}]), 802, 180, 340, 330)
b.part('dash', 'webexcel-chart', chart('月度订单数', 'column', ORDERS, '下单时间', [{'agg': 'count'}], 'month'), 270, 522, 520, 280)
b.part('dash', 'webexcel-table', table('月度销售额与同比', ORDERS, '下单时间', [{'column': '订单金额', 'agg': 'sum'}, {'column': '订单金额', 'agg': 'sum', 'show': 'yoy'}], top=0, sort='key', granularity='month'), 802, 522, 340, 280)
pivot_sheet(b, 'pv', '店铺 × 月份', ORDERS, '订单', {
    'rows': [{'column': '店铺', 'granularity': None}],
    'columns': [{'column': '下单时间', 'granularity': 'month'}],
    'values': [{'column': '订单金额', 'agg': 'sum'}, {'column': '订单金额', 'agg': 'sum', 'show': 'yoy'}],
}, '各店铺每月销售额与同比')
notes(b, 'help', '月度经营分析 · 使用说明', [
    '数据：一份订单明细，需要「店铺」「下单时间」「订单金额」三列（列名不同时在使用模板时对应即可）。',
    '经营看板：左侧切片器选择店铺、时间范围，指标卡、图表、排行同时联动；可点顶部「仪表盘 → 演示」全屏展示。',
    '店铺 × 月份：数据透视表，含每月同比；可在右侧透视面板继续调整。',
    '· 销售额同比按上年同期计算；订单数、客单价为环比（上月）。',
    '· 数据更新后（导入或数据连接同步），重新打开工作簿或点刷新即按最新数据计算。',
    '· 配合「定时推送」可每天把经营看板发到老板群。',
])
write('monthly-ops.json', {
    'id': 'builtin_monthly_ops', 'name': '月度经营分析', 'category': '经营分析',
    'description': '销售额、订单数、客单价及同比 / 环比，店铺排行与月度趋势，店铺 × 月份透视。适合老板看板与经营周会',
    'datasets': [orders_ds],
}, b)

# ── 2. 退款与售后分析（资金流水，内置数据集）──

LEDGER = 'ledger_detail'
REFUND = ['退款-结算后退款-退用户', '退款至用户']
AFTER_SALE = ['退换货运费险', '消费者赔付', '上门取件-支付快递费', '快递拦截费']
refund_filter = [{'op': 'in', 'column': '动账场景', 'values': REFUND}]
ledger_ds = {
    'key': LEDGER, 'name': '资金流水', 'builtinId': LEDGER, 'description': '平台内置的店铺资金流水（导入店铺明细后自动更新）',
    'columns': [
        {'title': '店铺', 'kind': 'text'},
        {'title': '动账时间', 'kind': 'datetime'},
        {'title': '动账金额', 'kind': 'money'},
        {'title': '动账场景', 'kind': 'text'},
        {'title': '商品名称', 'kind': 'text'},
    ],
}

b = Book('退款与售后分析')
b.sheet('dash', '退款看板', gridlines=False)
b.part('dash', 'webexcel-slicer', slicer('店铺', LEDGER, '店铺'), 58, 32, 200, 220)
b.part('dash', 'webexcel-slicer', slicer('动账时间', LEDGER, '动账时间', 'date'), 58, 264, 200, 170)
b.part('dash', 'webexcel-kpi', kpi('退款金额', LEDGER, '动账金额', 'sum', '动账时间', 'mom', False, refund_filter), 270, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('退款笔数', LEDGER, None, 'count', '动账时间', 'mom', False, refund_filter), 532, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('货款结算入账', LEDGER, '动账金额', 'sum', '动账时间', 'mom', True, [{'op': 'in', 'column': '动账场景', 'values': ['货款结算入账']}]), 794, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('运费险与赔付', LEDGER, '动账金额', 'sum', '动账时间', 'mom', False, [{'op': 'in', 'column': '动账场景', 'values': AFTER_SALE}]), 1056, 32, 250, 136)
b.part('dash', 'webexcel-chart', chart('每日退款金额', 'column', LEDGER, '动账时间', [{'column': '动账金额', 'agg': 'sum'}], 'day', split='店铺', filters=refund_filter, stacked=True), 270, 180, 520, 320)
b.part('dash', 'webexcel-table', table('退款金额按店铺', LEDGER, '店铺', [{'column': '动账金额', 'agg': 'sum'}, {'column': '动账金额', 'agg': 'sum', 'show': 'pct_total'}], filters=refund_filter), 802, 180, 504, 320)
b.part('dash', 'webexcel-table', table('退款最多的商品', LEDGER, '商品名称', [{'column': '动账金额', 'agg': 'sum'}, {'agg': 'count'}], filters=refund_filter), 270, 512, 520, 320)
b.part('dash', 'webexcel-table', table('退款相关流水构成', LEDGER, '动账场景', [{'column': '动账金额', 'agg': 'sum'}, {'agg': 'count'}], top=0, filters=[{'op': 'prefix', 'column': '动账场景', 'value': '退款'}]), 802, 512, 504, 320)
pivot_sheet(b, 'pv', '退款透视', LEDGER, '资金流水', {
    'rows': [{'column': '店铺', 'granularity': None}, {'column': '动账场景', 'granularity': None}],
    'columns': [{'column': '动账时间', 'granularity': 'month'}],
    'values': [{'column': '动账金额', 'agg': 'sum'}],
    'filters': [{'op': 'prefix', 'column': '动账场景', 'value': '退款'}],
}, '各店铺每月退款相关流水（按动账场景）')
notes(b, 'help', '退款与售后分析 · 使用说明', [
    '数据：平台内置的「资金流水」，导入店铺资金明细后自动更新，使用模板时无需选择数据。',
    '退款金额 = 动账场景为「退款-结算后退款-退用户」「退款至用户」的动账金额（退给消费者的钱）；上升为不利（红色）。',
    '运费险与赔付 = 退换货运费险、消费者赔付、上门取件快递费、快递拦截费。',
    '退款相关流水构成：所有以「退款」开头的动账场景，含退分账、退补贴等平台返还项，便于核对退款对结算的影响。',
    '· 结算入账时已扣减的部分退款（货款结算入账中的订单退款列）不计入退款金额。',
    '· 退款透视：店铺 × 动账场景 × 月份，可在右侧透视面板继续调整。',
])
write('refund-analysis.json', {
    'id': 'builtin_refund_analysis', 'name': '退款与售后分析', 'category': '电商财务',
    'description': '基于资金流水：退款金额与笔数环比、每日退款趋势、店铺与商品退款排行、运费险与赔付。无需准备数据',
    'datasets': [ledger_ds],
}, b)

# ── 3. 千川投放 ROI（订单 + 投放消耗）──

ADS = '$ads'
ads_ds = {
    'key': ADS, 'name': '投放消耗', 'description': '千川（或其他投放平台）的消耗明细，可从投放后台导出后导入',
    'columns': [
        {'title': '日期', 'kind': 'datetime', 'description': '消耗日期'},
        {'title': '店铺', 'kind': 'text', 'description': '与订单数据中的店铺名称一致'},
        {'title': '消耗', 'kind': 'money', 'description': '投放消耗金额'},
    ],
}

b = Book('千川投放 ROI')
b.sheet('roi', 'ROI 月报', col_widths={0: 80, 1: 70, 2: 120, 3: 120, 4: 96, 5: 80, 6: 80})
b.cell('roi', 0, 0, '千川投放 ROI 月报（最近 12 个月）', s='title')
b.cell('roi', 1, 0, '店铺', s='label')
b.cell('roi', 1, 1, '*', s='input')
b.cell('roi', 1, 2, '← 填店铺名只看该店铺；* 为全部店铺', s='note')
headers = ['月份', 'ROI', '成交金额', '投放消耗', '消耗占成交', '开始', '结束']
for c, h in enumerate(headers):
    b.cell('roi', 3, c, h, s='header')
first, last = 5, 16  # 第 5~16 行（0 起算 4~15）
def month_start(offset):
    """当前月份起第 offset 个月的 1 号（offset 为负表示之前）"""
    m = 'MONTH(TODAY())' + (f'+{offset}' if offset > 0 else f'-{-offset}' if offset < 0 else '')
    return f'=TEXT(DATE(YEAR(TODAY()),{m},1),"yyyy-mm-dd")'


for i in range(12):
    r = 4 + i
    n = r + 1  # 公式里的行号
    b.cell('roi', r, 5, f=month_start(i - 11), s='helper')
    b.cell('roi', r, 6, f=month_start(i - 10), s='helper')
    b.cell('roi', r, 0, f=f'=LEFT(F{n},7)', s='label')
    b.cell('roi', r, 2, f=f'=DSUMIFS("订单","订单金额","下单时间",">="&F{n},"下单时间","<"&G{n},"店铺",$B$2)', s='money')
    b.cell('roi', r, 3, f=f'=DSUMIFS("投放消耗","消耗","日期",">="&F{n},"日期","<"&G{n},"店铺",$B$2)', s='money')
    b.cell('roi', r, 1, f=f'=IF(D{n}=0,"-",C{n}/D{n})', s='ratio')
    b.cell('roi', r, 4, f=f'=IF(C{n}=0,"-",D{n}/C{n})', s='pct')
t = 16
b.cell('roi', t, 0, '合计', s='totalLabel')
b.cell('roi', t, 2, f=f'=SUM(C{first}:C{last})', s='totalMoney')
b.cell('roi', t, 3, f=f'=SUM(D{first}:D{last})', s='totalMoney')
b.cell('roi', t, 1, f=f'=IF(D{t + 1}=0,"-",C{t + 1}/D{t + 1})', s='totalRatio')
b.cell('roi', t, 4, f=f'=IF(C{t + 1}=0,"-",D{t + 1}/C{t + 1})', s='totalPct')
b.cell('roi', 18, 0, 'ROI = 成交金额 ÷ 投放消耗；成交金额为全部订单（含自然流量），如需只看投放带来的成交，可在订单数据中增加「流量来源」列后按需修改公式条件。', s='note')
# 图表放在表格右侧：ROI 趋势（单元格区域 A4:B16）
roi_left = HEAD_W + 80 + 70 + 120 + 120 + 96 + 80 + 80 + 24
b.part('roi', 'webexcel-chart', {'v': 1, 'type': 'line', 'title': '月度 ROI',
                                 'source': {'kind': 'range', 'sheetId': 'roi', 'range': 'A4:B16', 'headerRow': True, 'firstColLabels': True, 'seriesIn': 'columns'},
                                 'legend': 'top', 'labels': 'key', 'unit': 'raw'}, roi_left, 32, 480, 300)
b.part('roi', 'webexcel-kpi', kpi('本月投放消耗', ADS, '消耗', 'sum', '日期', 'mom', False), roi_left, 344, 234, 136)
b.part('roi', 'webexcel-kpi', kpi('本月成交金额', ORDERS, '订单金额', 'sum', '下单时间', 'mom'), roi_left + 246, 344, 234, 136)
b.part('roi', 'webexcel-chart', chart('各店铺月度投放消耗', 'column', ADS, '日期', [{'column': '消耗', 'agg': 'sum'}], 'month', split='店铺', stacked=True), roi_left, 492, 480, 300)
notes(b, 'help', '千川投放 ROI · 使用说明', [
    '数据：订单明细（店铺、下单时间、订单金额）与投放消耗（日期、店铺、消耗）两份数据，店铺名称需一致。',
    'ROI 月报：最近 12 个月按月汇总成交、消耗与 ROI，月份随当前日期自动滚动；B2 填店铺名只看该店铺。',
    '公式使用 DSUMIFS 在服务端汇总千万行数据，可照此增加列（如退款后成交、净 ROI）。',
    '· 「开始」「结束」两列是公式用的月份边界，请勿删除。',
    '· 右侧指标卡为本月数据与上月对比；柱状图为各店铺每月消耗。',
])
write('qianchuan-roi.json', {
    'id': 'builtin_qianchuan_roi', 'name': '千川投放 ROI', 'category': '电商财务',
    'description': '订单成交与千川投放消耗按月对比：ROI、消耗占成交、各店铺消耗趋势；月份自动滚动，可按店铺查看',
    'datasets': [orders_ds, ads_ds],
}, b)

# ── 4. 多平台利润表（经营明细）──

PNL = '$pnl'
pnl_ds = {
    'key': PNL, 'name': '经营明细', 'description': '按平台、按日的收入与成本费用明细（每行一个平台一天，或一笔订单 / 一笔费用），可来自导入或数据连接',
    'columns': [
        {'title': '平台', 'kind': 'text', 'description': '销售平台 / 店铺，如 抖店、淘宝、拼多多'},
        {'title': '日期', 'kind': 'datetime', 'description': '业务日期'},
        {'title': '销售收入', 'kind': 'money', 'description': '确认的销售收入（退款后净额也可）'},
        {'title': '销售成本', 'kind': 'money', 'description': '对应商品成本'},
        {'title': '平台费用', 'kind': 'money', 'description': '平台佣金、技术服务费、运费等'},
        {'title': '推广费用', 'kind': 'money', 'description': '投流、达人佣金等推广支出'},
    ],
}

b = Book('多平台利润表')
b.sheet('dash', '利润看板', gridlines=False)
b.part('dash', 'webexcel-slicer', slicer('平台', PNL, '平台'), 58, 32, 200, 260)
b.part('dash', 'webexcel-slicer', slicer('日期', PNL, '日期', 'date', 'this_year'), 58, 304, 200, 170)
b.part('dash', 'webexcel-kpi', kpi('销售收入', PNL, '销售收入', 'sum', '日期', 'yoy'), 270, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('销售成本', PNL, '销售成本', 'sum', '日期', 'mom', False), 532, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('平台费用', PNL, '平台费用', 'sum', '日期', 'mom', False), 794, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('推广费用', PNL, '推广费用', 'sum', '日期', 'mom', False), 1056, 32, 250, 136)
b.part('dash', 'webexcel-chart', chart('各平台月度销售收入', 'line', PNL, '日期', [{'column': '销售收入', 'agg': 'sum'}], 'month', split='平台'), 270, 180, 520, 330)
b.part('dash', 'webexcel-table', table('平台收入排行', PNL, '平台', [{'column': '销售收入', 'agg': 'sum'}, {'column': '销售收入', 'agg': 'sum', 'show': 'pct_total'}]), 802, 180, 504, 330)
b.part('dash', 'webexcel-chart', chart('各平台推广费用', 'column', PNL, '日期', [{'column': '推广费用', 'agg': 'sum'}], 'month', split='平台', stacked=True), 270, 522, 520, 280)
b.part('dash', 'webexcel-table', table('月度收入与同比', PNL, '日期', [{'column': '销售收入', 'agg': 'sum'}, {'column': '销售收入', 'agg': 'sum', 'show': 'yoy'}], top=0, sort='key', granularity='month'), 802, 522, 504, 280)

b.sheet('pnl', '利润月报', col_widths={0: 80, 1: 110, 2: 110, 3: 110, 4: 80, 5: 100, 6: 100, 7: 110, 8: 80, 9: 80, 10: 80})
b.cell('pnl', 0, 0, '多平台利润月报（最近 12 个月）', s='title')
b.cell('pnl', 1, 0, '平台', s='label')
b.cell('pnl', 1, 1, '*', s='input')
b.cell('pnl', 1, 2, '← 填平台名只看该平台；* 为全部平台', s='note')
for c, h in enumerate(['月份', '销售收入', '销售成本', '毛利', '毛利率', '平台费用', '推广费用', '净利润', '净利率', '开始', '结束']):
    b.cell('pnl', 3, c, h, s='header')


def pnl_sum(col, n):
    return f'=DSUMIFS("经营明细","{col}","日期",">="&J{n},"日期","<"&K{n},"平台",$B$2)'


for i in range(12):
    r, n = 4 + i, 5 + i
    b.cell('pnl', r, 9, f=month_start(i - 11), s='helper')
    b.cell('pnl', r, 10, f=month_start(i - 10), s='helper')
    b.cell('pnl', r, 0, f=f'=LEFT(J{n},7)', s='label')
    b.cell('pnl', r, 1, f=pnl_sum('销售收入', n), s='money')
    b.cell('pnl', r, 2, f=pnl_sum('销售成本', n), s='money')
    b.cell('pnl', r, 3, f=f'=B{n}-C{n}', s='money')
    b.cell('pnl', r, 4, f=f'=IF(B{n}=0,"-",D{n}/B{n})', s='pct')
    b.cell('pnl', r, 5, f=pnl_sum('平台费用', n), s='money')
    b.cell('pnl', r, 6, f=pnl_sum('推广费用', n), s='money')
    b.cell('pnl', r, 7, f=f'=D{n}-F{n}-G{n}', s='money')
    b.cell('pnl', r, 8, f=f'=IF(B{n}=0,"-",H{n}/B{n})', s='pct')
b.cell('pnl', 16, 0, '合计', s='totalLabel')
for c, col in ((1, 'B'), (2, 'C'), (3, 'D'), (5, 'F'), (6, 'G'), (7, 'H')):
    b.cell('pnl', 16, c, f=f'=SUM({col}5:{col}16)', s='totalMoney')
b.cell('pnl', 16, 4, f='=IF(B17=0,"-",D17/B17)', s='totalPct')
b.cell('pnl', 16, 8, f='=IF(B17=0,"-",H17/B17)', s='totalPct')
b.cell('pnl', 18, 0, '毛利 = 销售收入 - 销售成本；净利润 = 毛利 - 平台费用 - 推广费用（不含人工、房租等未在数据里的费用）。', s='note')
pnl_left = HEAD_W + 80 + 110 + 110 + 110 + 80 + 100 + 100 + 110 + 80 + 80 + 80 + 24
b.part('pnl', 'webexcel-chart', {'v': 1, 'type': 'column', 'title': '月度收入、成本与毛利',
                                 'source': {'kind': 'range', 'sheetId': 'pnl', 'range': 'A4:D16', 'headerRow': True, 'firstColLabels': True, 'seriesIn': 'columns'},
                                 'legend': 'top', 'labels': 'none', 'unit': 'auto'}, pnl_left, 32, 520, 320)
pivot_sheet(b, 'pv', '平台 × 月份', PNL, '经营明细', {
    'rows': [{'column': '平台', 'granularity': None}],
    'columns': [{'column': '日期', 'granularity': 'month'}],
    'values': [{'column': '销售收入', 'agg': 'sum'}, {'column': '销售成本', 'agg': 'sum'}, {'column': '平台费用', 'agg': 'sum'}, {'column': '推广费用', 'agg': 'sum'}],
}, '各平台每月收入、成本与费用')
notes(b, 'help', '多平台利润表 · 使用说明', [
    '数据：一份经营明细，需要「平台」「日期」「销售收入」「销售成本」「平台费用」「推广费用」六列（没有的费用列可以整列填 0；列名不同时在使用模板时对应即可）。',
    '利润看板：左侧切片器选平台、时间范围，指标卡、图表、排行联动；顶部「仪表盘 → 演示」可全屏展示。',
    '利润月报：最近 12 个月按月汇总收入、成本、毛利、费用与净利润，月份随当前日期自动滚动；B2 填平台名只看该平台。',
    '平台 × 月份：数据透视表，对比各平台每月收入与成本费用，可在右侧透视面板继续调整。',
    '· 「开始」「结束」两列是公式用的月份边界，请勿删除。',
    '· 利润只含数据里有的收入与费用；人工、房租、税费等需要另行加列或在公式里扣减。',
    '· 配合「定时推送」可每月把利润看板发到老板群。',
])
write('platform-pnl.json', {
    'id': 'builtin_platform_pnl', 'name': '多平台利润表', 'category': '电商财务',
    'description': '各平台销售收入、成本、毛利、平台与推广费用、净利润：看板 + 最近 12 个月利润月报 + 平台 × 月份透视。月份自动滚动，可按平台查看',
    'datasets': [pnl_ds],
}, b)

# ── 5. 应收应付对账（往来明细）──

AR = '$partners'
ar_ds = {
    'key': AR, 'name': '往来明细', 'description': '与客户 / 供应商的往来流水，每行一笔：应收、收款、应付、付款（类型列取这四个值之一）',
    'columns': [
        {'title': '往来单位', 'kind': 'text', 'description': '客户或供应商名称'},
        {'title': '日期', 'kind': 'datetime', 'description': '业务发生日期'},
        {'title': '类型', 'kind': 'text', 'description': '只能是：应收、收款、应付、付款'},
        {'title': '金额', 'kind': 'money', 'description': '正数金额（方向由类型表示）'},
    ],
}


def typ(*values):
    return [{'op': 'in', 'column': '类型', 'values': list(values)}]


b = Book('应收应付对账')
b.sheet('dash', '往来看板', gridlines=False)
b.part('dash', 'webexcel-slicer', slicer('往来单位', AR, '往来单位'), 58, 32, 200, 300)
b.part('dash', 'webexcel-slicer', slicer('日期', AR, '日期', 'date', 'this_year'), 58, 344, 200, 170)
b.part('dash', 'webexcel-kpi', kpi('应收发生', AR, '金额', 'sum', '日期', 'mom', True, typ('应收')), 270, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('已收款', AR, '金额', 'sum', '日期', 'mom', True, typ('收款')), 532, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('应付发生', AR, '金额', 'sum', '日期', 'mom', False, typ('应付')), 794, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('已付款', AR, '金额', 'sum', '日期', 'mom', False, typ('付款')), 1056, 32, 250, 136)
b.part('dash', 'webexcel-chart', chart('月度应收与收款', 'column', AR, '日期', [{'column': '金额', 'agg': 'sum'}], 'month', split='类型', filters=typ('应收', '收款')), 270, 180, 520, 330)
b.part('dash', 'webexcel-chart', chart('月度应付与付款', 'column', AR, '日期', [{'column': '金额', 'agg': 'sum'}], 'month', split='类型', filters=typ('应付', '付款')), 802, 180, 504, 330)
b.part('dash', 'webexcel-table', table('应收最多的客户', AR, '往来单位', [{'column': '金额', 'agg': 'sum'}, {'agg': 'count'}], filters=typ('应收')), 270, 522, 520, 300)
b.part('dash', 'webexcel-table', table('应付最多的供应商', AR, '往来单位', [{'column': '金额', 'agg': 'sum'}, {'agg': 'count'}], filters=typ('应付')), 802, 522, 504, 300)

b.sheet('bal', '往来余额月报', col_widths={0: 80, 1: 110, 2: 110, 3: 110, 4: 110, 5: 80, 6: 80})
b.cell('bal', 0, 0, '往来余额月报（最近 12 个月）', s='title')
b.cell('bal', 1, 0, '往来单位', s='label')
b.cell('bal', 1, 1, '*', s='input')
b.cell('bal', 1, 2, '← 填客户 / 供应商名只看该单位；* 为全部', s='note')


def roll(top, title, add, sub, add_label, sub_label):
    """余额滚动表：期初 + 本期增加 - 本期减少 = 期末。top 为标题所在行（0 起算）"""
    b.cell('bal', top, 0, title, s='title')
    hr = top + 1
    for c, h in enumerate(['月份', '期初余额', add_label, sub_label, '期末余额', '开始', '结束']):
        b.cell('bal', hr, c, h, s='header')
    first = hr + 2  # 第一个数据行的行号（1 起算）
    for i in range(12):
        r, n = hr + 1 + i, hr + 2 + i
        b.cell('bal', r, 5, f=month_start(i - 11), s='helper')
        b.cell('bal', r, 6, f=month_start(i - 10), s='helper')
        b.cell('bal', r, 0, f=f'=LEFT(F{n},7)', s='label')
        before = lambda t: f'DSUMIFS("往来明细","金额","日期","<"&F{n},"类型","{t}","往来单位",$B$2)'
        during = lambda t: f'DSUMIFS("往来明细","金额","日期",">="&F{n},"日期","<"&G{n},"类型","{t}","往来单位",$B$2)'
        b.cell('bal', r, 1, f=f'={before(add)}-{before(sub)}', s='money')
        b.cell('bal', r, 2, f=f'={during(add)}', s='money')
        b.cell('bal', r, 3, f=f'={during(sub)}', s='money')
        b.cell('bal', r, 4, f=f'=B{n}+C{n}-D{n}', s='money')
    t = hr + 13
    last = hr + 13  # 最后一个数据行的行号（1 起算）
    b.cell('bal', t, 0, '合计', s='totalLabel')
    b.cell('bal', t, 1, f=f'=B{first}', s='totalMoney')
    b.cell('bal', t, 2, f=f'=SUM(C{first}:C{last})', s='totalMoney')
    b.cell('bal', t, 3, f=f'=SUM(D{first}:D{last})', s='totalMoney')
    b.cell('bal', t, 4, f=f'=E{last}', s='totalMoney')


roll(3, '应收往来：期初 + 应收发生 - 收款 = 期末应收余额', '应收', '收款', '本期应收', '本期收款')
roll(21, '应付往来：期初 + 应付发生 - 付款 = 期末应付余额', '应付', '付款', '本期应付', '本期付款')
b.cell('bal', 39, 0, '期初余额 = 该月 1 号之前累计的 增加 - 减少（含更早年份的数据）；期末余额为正表示对方欠我们（应收）/ 我们欠对方（应付）。', s='note')
pivot_sheet(b, 'pv', '往来单位 × 类型', AR, '往来明细', {
    'rows': [{'column': '往来单位', 'granularity': None}],
    'columns': [{'column': '类型', 'granularity': None}],
    'values': [{'column': '金额', 'agg': 'sum'}],
}, '各往来单位的应收、收款、应付、付款合计')
notes(b, 'help', '应收应付对账 · 使用说明', [
    '数据：一份往来明细，需要「往来单位」「日期」「类型」「金额」四列；类型只能取「应收」「收款」「应付」「付款」，金额填正数。',
    '往来看板：左侧切片器选往来单位、时间范围；指标卡为本月应收 / 收款 / 应付 / 付款及环比；下方是月度对比与排行。',
    '往来余额月报：最近 12 个月的期初 + 本期增加 - 本期减少 = 期末余额，应收、应付各一张；B2 填往来单位名只看该单位，* 为全部。',
    '往来单位 × 类型：数据透视表，一眼看出每个单位的应收、收款、应付、付款合计，差额即未结余额。',
    '· 余额按「累计增加 - 累计减少」计算，不区分具体哪笔收款冲销哪笔应收（不是账龄分析）。',
    '· 「开始」「结束」两列是公式用的月份边界，请勿删除。',
    '· 对账时：把对方的对账单也导入为一份往来明细，用往来单位 × 类型透视分别核对两边的期末余额。',
])
write('ar-ap.json', {
    'id': 'builtin_ar_ap', 'name': '应收应付对账', 'category': '电商财务',
    'description': '客户应收 / 收款、供应商应付 / 付款：往来看板、最近 12 个月余额滚动表（期初 + 增加 - 减少 = 期末）、往来单位 × 类型透视，用于对账',
    'datasets': [ar_ds],
}, b)

# ── 6. 库存进销存（库存流水）──

STK = '$stock'
stock_ds = {
    'key': STK, 'name': '库存流水', 'description': '出入库流水，每行一笔：期初（期初结存录入）、入库、出库（单据类型列取这三个值之一）',
    'columns': [
        {'title': '商品', 'kind': 'text', 'description': '商品名称或 SKU'},
        {'title': '日期', 'kind': 'datetime', 'description': '业务发生日期'},
        {'title': '单据类型', 'kind': 'text', 'description': '只能是：期初、入库、出库'},
        {'title': '数量', 'kind': 'number', 'description': '正数数量（方向由单据类型表示）'},
    ],
}


def doc(*values):
    return [{'op': 'in', 'column': '单据类型', 'values': list(values)}]


b = Book('库存进销存')
b.sheet('dash', '库存看板', gridlines=False)
b.part('dash', 'webexcel-slicer', slicer('商品', STK, '商品'), 58, 32, 200, 300)
b.part('dash', 'webexcel-slicer', slicer('日期', STK, '日期', 'date', 'this_year'), 58, 344, 200, 170)
b.part('dash', 'webexcel-kpi', kpi('入库数量', STK, '数量', 'sum', '日期', 'mom', True, doc('入库')), 270, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('出库数量', STK, '数量', 'sum', '日期', 'mom', True, doc('出库')), 532, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('入库笔数', STK, None, 'count', '日期', 'mom', True, doc('入库')), 794, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('出库笔数', STK, None, 'count', '日期', 'mom', True, doc('出库')), 1056, 32, 250, 136)
b.part('dash', 'webexcel-chart', chart('月度入库与出库', 'column', STK, '日期', [{'column': '数量', 'agg': 'sum'}], 'month', split='单据类型', filters=doc('入库', '出库')), 270, 180, 520, 330)
b.part('dash', 'webexcel-chart', chart('月度出库趋势', 'line', STK, '日期', [{'column': '数量', 'agg': 'sum'}], 'month', filters=doc('出库')), 802, 180, 504, 330)
b.part('dash', 'webexcel-table', table('出库最多的商品', STK, '商品', [{'column': '数量', 'agg': 'sum'}, {'agg': 'count'}], filters=doc('出库')), 270, 522, 520, 300)
b.part('dash', 'webexcel-table', table('入库最多的商品', STK, '商品', [{'column': '数量', 'agg': 'sum'}, {'agg': 'count'}], filters=doc('入库')), 802, 522, 504, 300)

b.sheet('bal', '库存月报', col_widths={0: 80, 1: 110, 2: 110, 3: 110, 4: 110, 5: 80, 6: 80})
b.cell('bal', 0, 0, '库存月报（最近 12 个月）', s='title')
b.cell('bal', 1, 0, '商品', s='label')
b.cell('bal', 1, 1, '*', s='input')
b.cell('bal', 1, 2, '← 填商品名只看该商品；* 为全部', s='note')
b.cell('bal', 3, 0, '期初库存 + 本期入库 - 本期出库 = 期末库存', s='title')
for c, h in enumerate(['月份', '期初库存', '本期入库', '本期出库', '期末库存', '开始', '结束']):
    b.cell('bal', 4, c, h, s='header')
for i in range(12):
    r, n = 5 + i, 6 + i
    b.cell('bal', r, 5, f=month_start(i - 11), s='helper')
    b.cell('bal', r, 6, f=month_start(i - 10), s='helper')
    b.cell('bal', r, 0, f=f'=LEFT(F{n},7)', s='label')
    before = lambda t: f'DSUMIFS("库存流水","数量","日期","<"&F{n},"单据类型","{t}","商品",$B$2)'
    during = lambda t: f'DSUMIFS("库存流水","数量","日期",">="&F{n},"日期","<"&G{n},"单据类型","{t}","商品",$B$2)'
    b.cell('bal', r, 1, f=f'={before("期初")}+{before("入库")}-{before("出库")}', s='qty')
    # 期初录入（单据类型 = 期初）落在本月的，算进本期入库
    b.cell('bal', r, 2, f=f'={during("入库")}+{during("期初")}', s='qty')
    b.cell('bal', r, 3, f=f'={during("出库")}', s='qty')
    b.cell('bal', r, 4, f=f'=B{n}+C{n}-D{n}', s='qty')
b.cell('bal', 17, 0, '合计', s='totalLabel')
b.cell('bal', 17, 1, f='=B6', s='totalQty')
b.cell('bal', 17, 2, f='=SUM(C6:C17)', s='totalQty')
b.cell('bal', 17, 3, f='=SUM(D6:D17)', s='totalQty')
b.cell('bal', 17, 4, f='=E17', s='totalQty')
b.cell('bal', 19, 0, '期初库存 = 该月 1 号之前累计的（期初 + 入库 - 出库），含更早年份的数据；单据类型为「期初」的记录落在本月的算进本期入库。', s='note')
pivot_sheet(b, 'pv', '商品 × 单据类型', STK, '库存流水', {
    'rows': [{'column': '商品', 'granularity': None}],
    'columns': [{'column': '单据类型', 'granularity': None}],
    'values': [{'column': '数量', 'agg': 'sum'}],
    'calcItems': [{'field': '单据类型', 'name': '期末库存', 'formula': '[期初] + [入库] - [出库]'}],
}, '各商品的期初、入库、出库与期末库存（期末库存 = 期初 + 入库 - 出库）')
notes(b, 'help', '库存进销存 · 使用说明', [
    '数据：一份库存流水，需要「商品」「日期」「单据类型」「数量」四列；单据类型只能取「期初」「入库」「出库」，数量填正数。',
    '库存看板：左侧切片器选商品、时间范围；指标卡为本月入库 / 出库数量与笔数及环比；下方是月度入库与出库对比、出库趋势、入库 / 出库排行。',
    '库存月报：最近 12 个月的期初库存 + 本期入库 - 本期出库 = 期末库存；B2 填商品名只看该商品，* 为全部。',
    '商品 × 单据类型：数据透视表，每个商品一行，列出期初、入库、出库合计，最后一列「期末库存」是计算项（期初 + 入库 - 出库）；某个商品没有出库记录时按 0 算。',
    '· 透视最右边的「总计」列是期初、入库、出库的数量直接相加，在库存里没有意义，看「期末库存」那一列即可；最下面的总计行是各列的合计。',
    '· 期初库存只在第一次盘点时录入一次（单据类型填「期初」，日期填盘点日），之后只记入库和出库。',
    '· 库存月报和透视里的「期末库存」是按流水累计算出来的，不是仓库实际盘点数；盘点有差异时请补一笔入库或出库来调整。',
    '· 「开始」「结束」两列是公式用的月份边界，请勿删除。',
    '· 数量不分单位（件 / 箱）；同一商品请统一单位。',
])
write('stock.json', {
    'id': 'builtin_stock', 'name': '库存进销存', 'category': '电商运营',
    'description': '出入库流水：库存看板、最近 12 个月库存月报（期初 + 入库 - 出库 = 期末）、商品 × 单据类型透视（期末库存为计算项）',
    'datasets': [stock_ds],
}, b)

# ── 7. 银行余额调节表（对账结果）──
# 数据来自「新建对账」生成的结果数据集（A 方银行流水、B 方企业账）：每行带来源、日期、金额、状态（已勾对 / 未勾对）、勾对完成日、勾对差额

REC = '$rec'
rec_ds = {
    'key': REC, 'name': '对账结果', 'description': '逐笔对账的结果数据集（在「新建对账」里把银行流水设为 A 方、企业账设为 B 方）：每行一笔，带状态、勾对完成日、勾对差额',
    'columns': [
        {'title': '来源', 'kind': 'text', 'description': '对账里两方的显示名称（如：银行、账面）'},
        {'title': '日期', 'kind': 'datetime', 'description': '这一笔的日期'},
        {'title': '金额', 'kind': 'money', 'description': '已统一符号：收入为正、支出为负'},
        {'title': '方向', 'kind': 'text', 'description': '收入 / 支出（按金额正负）'},
        {'title': '状态', 'kind': 'text', 'description': '已勾对 / 未勾对'},
        {'title': '勾对完成日', 'kind': 'datetime', 'description': '已勾对的组里最晚的日期（对方日期晚于截至日时，这一笔在截至日仍是未达账项）'},
        {'title': '勾对差额', 'kind': 'money', 'description': '勾对时确认的差额（A 方合计 - B 方合计），每组记在一行上'},
        {'title': '摘要', 'kind': 'text', 'description': '对账里选的摘要列'},
    ],
}


def rec_state(*values):
    return [{'op': 'in', 'column': '状态', 'values': list(values)}]


b = Book('银行余额调节表')
b.sheet('adj', '调节表', col_widths={0: 250, 1: 120, 2: 60, 3: 380})
b.cell('adj', 0, 0, '银行存款余额调节表', s='title')
inputs = [
    (1, '银行方名称', '银行', None, '← 对账里 A 方的「显示名称」（结果数据集「来源」列里银行流水的名字）'),
    (2, '企业方名称', '账面', None, '← 对账里 B 方的「显示名称」'),
    (3, '截至日期', None, '=TEXT(TODAY()-DAY(TODAY()),"yyyy-mm-dd")', '← 默认上月末；改成要调节的日期，格式 2026-09-30'),
    (4, '银行期初余额', 0, None, '← 对账数据开始日之前，银行对账单上的余额'),
    (5, '企业期初余额', 0, None, '← 对账数据开始日之前，企业银行存款账上的余额'),
]
for r, label, v, f, hint in inputs:
    b.cell('adj', r, 0, label, s='label')
    b.cell('adj', r, 1, v=v, f=f, s='input')
    b.cell('adj', r, 3, hint, s='note')
b.cell('adj', 6, 0, '截至日期（公式用）', s='helper')
b.cell('adj', 6, 1, f='=IF(ISNUMBER(B4),TEXT(B4,"yyyy-mm-dd"),B4)', s='helper')

# 公式里的片段：$B$2 银行方名称、$B$3 企业方名称、$B$7 截至日期
asof = '"日期","<="&$B$7'


def rows_of(who, cond):
    """截至日，来源为 who 的行里满足 cond（金额条件）的未达账项：未勾对的，加上已勾对但对方日期晚于截至日的"""
    return who, cond


def out_sum(who, op):
    """截至日，来源为 who、金额满足 op 的未达账项合计：未勾对的，加上已勾对但对方日期晚于截至日的"""
    a = f'DSUMIFS("对账结果","金额","来源",{who},{asof},"金额","{op}","状态","未勾对")'
    c = f'DSUMIFS("对账结果","金额","来源",{who},{asof},"金额","{op}","状态","已勾对","勾对完成日",">"&$B$7)'
    return f'{a}+{c}'


def out_cnt(who, op):
    a = f'DCOUNTIFS("对账结果","来源",{who},{asof},"金额","{op}","状态","未勾对")'
    c = f'DCOUNTIFS("对账结果","来源",{who},{asof},"金额","{op}","状态","已勾对","勾对完成日",">"&$B$7)'
    return f'{a}+{c}'


for c, h in enumerate(['项目', '金额', '笔数', '说明']):
    b.cell('adj', 8, c, h, s='header')
lines = [
    # (行, 标签, 金额公式, 笔数公式, 样式, 说明)
    (9, '银行对账单余额', f'=B5+DSUMIFS("对账结果","金额","来源",$B$2,{asof})', f'=DCOUNTIFS("对账结果","来源",$B$2,{asof})', 'money', '期初余额 + 截至日的银行流水合计'),
    (10, '加：企业已收、银行未收', f'={out_sum("$B$3", ">0")}', f'={out_cnt("$B$3", ">0")}', 'money', '企业账上已记收入，银行还没入账（在途存款）'),
    (11, '减：企业已付、银行未付', f'=-({out_sum("$B$3", "<0")})', f'={out_cnt("$B$3", "<0")}', 'money', '企业账上已记支出，银行还没扣款（已开出未兑付的支票）'),
    (12, '调整后的银行存款余额', '=B10+B11-B12', None, 'totalMoney', ''),
    (14, '企业银行存款账面余额', f'=B6+DSUMIFS("对账结果","金额","来源",$B$3,{asof})', f'=DCOUNTIFS("对账结果","来源",$B$3,{asof})', 'money', '期初余额 + 截至日的企业账合计'),
    (15, '加：银行已收、企业未收', f'={out_sum("$B$2", ">0")}', f'={out_cnt("$B$2", ">0")}', 'money', '银行已入账，企业账上还没记（如利息、客户直接汇款）'),
    (16, '减：银行已付、企业未付', f'=-({out_sum("$B$2", "<0")})', f'={out_cnt("$B$2", "<0")}', 'money', '银行已扣款，企业账上还没记（如手续费）'),
    (17, '调整后的企业账面余额', '=B15+B16-B17', None, 'totalMoney', ''),
]
for r, label, fm, fc, st, hint in lines:
    total = st == 'totalMoney'
    b.cell('adj', r, 0, label, s='totalLabel' if total else 'label')
    b.cell('adj', r, 1, f=fm, s=st)
    if fc:
        b.cell('adj', r, 2, f=fc, s='qty')
    elif total:
        b.cell('adj', r, 2, s='totalQty')
    b.cell('adj', r, 3, hint, s='note')
for c, h in enumerate(['核对', '金额', '', '说明']):
    b.cell('adj', 19, c, h, s='header')
checks = [
    (20, '两个调整后余额的差额（银行 - 企业）', '=B13-B18', '应为 0；不为 0 时看下面两项能否解释'),
    (21, '其中：期初余额差异（银行 - 企业）', '=B5-B6', '两个期初余额不一致造成的'),
    (22, '其中：勾对时确认的差额', f'=DSUMIFS("对账结果","勾对差额","勾对完成日","<="&$B$7)', '手工勾对时合计不相等、写明原因后确认的差额（如银行手续费、汇率差），A 方 - B 方'),
    (23, '无法解释的差额', '=B21-B22-B23', '应为 0；不为 0 说明有的行没有日期、或对账数据不完整（见使用说明）'),
]
for r, label, fm, hint in checks:
    b.cell('adj', r, 0, label, s='label')
    b.cell('adj', r, 1, f=fm, s='money')
    b.cell('adj', r, 3, hint, s='note')
b.cell('adj', 24, 0, '结论', s='totalLabel')
b.cell('adj', 24, 1, f='=IF(ABS(B24)<0.005,IF(ABS(B21)<0.005,"✓ 已平衡","✓ 差额均已解释"),"✗ 有无法解释的差额")', s='totalLabel')
b.cell('adj', 24, 2, s='totalLabel')
b.cell('adj', 24, 3, s='totalLabel')
b.cell('adj', 26, 0, '未达账项 = 截至日仍未勾对的行，加上已勾对、但对方的日期晚于截至日的行（如 9 月 30 日开出的支票，银行 10 月 2 日才扣款）。', s='note')

b.sheet('dash', '对账看板', gridlines=False)
b.part('dash', 'webexcel-slicer', slicer('来源', REC, '来源'), 58, 32, 200, 200)
b.part('dash', 'webexcel-slicer', slicer('日期', REC, '日期', 'date', 'this_year'), 58, 244, 200, 170)
b.part('dash', 'webexcel-kpi', kpi('未勾对笔数', REC, None, 'count', '日期', 'mom', False, rec_state('未勾对')), 270, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('未勾对金额（净）', REC, '金额', 'sum', '日期', 'mom', False, rec_state('未勾对')), 532, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('已勾对笔数', REC, None, 'count', '日期', 'mom', True, rec_state('已勾对')), 794, 32, 250, 136)
b.part('dash', 'webexcel-kpi', kpi('勾对差额合计', REC, '勾对差额', 'sum', '日期', 'mom', False, rec_state('已勾对')), 1056, 32, 250, 136)
b.part('dash', 'webexcel-chart', chart('月度未勾对金额', 'column', REC, '日期', [{'column': '金额', 'agg': 'sum'}], 'month', split='来源', filters=rec_state('未勾对')), 270, 180, 520, 330)
b.part('dash', 'webexcel-chart', chart('月度已勾对笔数', 'column', REC, '日期', [{'agg': 'count'}], 'month', split='来源', filters=rec_state('已勾对')), 802, 180, 504, 330)
b.part('dash', 'webexcel-table', table('未勾对金额最大的摘要', REC, '摘要', [{'column': '金额', 'agg': 'sum'}, {'agg': 'count'}], filters=rec_state('未勾对')), 270, 522, 1036, 300)

pivot_sheet(b, 'pv', '未达账项汇总', REC, '对账结果', {
    'filters': rec_state('未勾对'),
    'rows': [{'column': '来源', 'granularity': None}],
    'columns': [{'column': '方向', 'granularity': None}],
    'values': [{'column': '金额', 'agg': 'sum'}, {'column': '金额', 'agg': 'count'}],
}, '未勾对的行按来源与收支方向汇总（不含已勾对但对方日期晚于截至日的，那部分以调节表为准）')
notes(b, 'help', '银行余额调节表 · 使用说明', [
    '先做逐笔对账：数据面板「新建对账」，A 方选银行流水、B 方选企业银行存款账，对应金额 / 日期 / 摘要列（有流水号、参考号等可作为对账键），自动勾对后把剩下的手工勾对。会生成一个对账结果数据集，套用本模板时选它。',
    '调节表：B2、B3 填两方在对账里的「显示名称」；B4 截至日期（默认上月末，格式 2026-09-30）；B5、B6 填两边的期初余额（对账数据开始日之前的余额；数据从开账日开始时填 0）。',
    '调节表按标准格式：银行对账单余额 + 企业已收银行未收 - 企业已付银行未付 = 调整后的银行存款余额；企业账面余额 + 银行已收企业未收 - 银行已付企业未付 = 调整后的企业账面余额；两者应相等。',
    '核对区：差额应能由「期初余额差异」与「勾对时确认的差额」解释，「无法解释的差额」应为 0。',
    '· A 方（银行）必须是对账里的第一方：「勾对差额」按 A 方 - B 方计算，两方放反了「无法解释的差额」会不为 0。',
    '· 没有日期的行不在任何截至日范围内（对账里选了日期列时才有这个问题），会让「无法解释的差额」不为 0：请补全日期。',
    '· 对账数据改了（重新导入、改了金额）后，调节表自动按最新结果计算；被改动的行原来的勾对会失效，回到未勾对。',
    '· 对账看板：未勾对 / 已勾对的笔数与金额、按月分布、未勾对金额最大的摘要；「未达账项汇总」页签是未勾对行的透视。',
    '· 笔数是未达账项的笔数；金额都按收入为正、支出为负的统一符号。',
])
write('bank-rec.json', {
    'id': 'builtin_bank_rec', 'name': '银行余额调节表', 'category': '电商财务',
    'description': '银行流水与企业账逐笔对账后，自动出具银行存款余额调节表（未达账项 4 项 + 差额核对）；含对账看板与未达账项透视',
    'datasets': [rec_ds],
}, b)
