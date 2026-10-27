/**
 * 店铺资金汇总表模板
 *
 * 版式：A1 月份 / 第 1 行商户主体（同主体的店铺合并单元格）/ 第 2 行店铺 / 第 3~27 行汇总项 / 底部核对区。
 * 每个数据单元格都是 DSUMIFS 公式，直接对服务端的「资金流水」数据集（千万行）按 主体 × 店铺 × 月份 × 场景 求和
 * （见 features/excel/formula/dataset-functions.ts）。A1 是月份（如 2026-10），改 A1 即整表按新月份重算；
 * 导入新数据后汇总表自动刷新。
 *
 * 符号约定：
 * - 收入区按资金影响取值（入账为正、出账为负），如小额打款、消费者赔付通常为负数
 * - 支出区按成本取值（扣款为正、退回为负），如佣金返还为负数
 * - 实际入账金额 = 收入合计 − 支出合计；余额变化 = 实际入账金额 − 提现
 */

export interface SummaryColumn {
  /** 商户主体全称（公式按此精确匹配「商户主体名称」） */
  entityFull: string
  /** 第 1 行显示的主体简称（可在表格中手动修改） */
  entityShort: string
  /** 店铺名称（第 2 行，同时作为公式的店铺条件） */
  shop: string
}

/** 汇总表固定行（0 起） */
export const SUMMARY_ROW = {
  entity: 0,
  shop: 1,
  /** 核对区：商户主体全称 */
  entityFull: 29,
} as const

export const SUMMARY_SHEET_NAME = '汇总表'
/** 汇总公式引用的数据集（服务端大数据表） */
export const LEDGER_DATASET = '资金流水'
/** 旧版的「流水汇总数据」页签（公式下推后不再需要，加载时移除） */
export const LEGACY_PIVOT_SHEET_NAME = '流水汇总数据'

/**
 * 归类规则项（声明式）：同一份规则既生成汇总表的 SUMIFS 公式，也作为明细下钻条件发给后端，
 * 因此「明细贡献合计」必然等于汇总表单元格的值
 */
/** 某金额列合计；op 只取负数/正数部分（如佣金扣款与佣金退回分开归类） */
export interface ColumnTerm { kind: 'column', column: string, op?: 'negative' | 'positive', sign: 1 | -1 }
/** 某动账场景（Excel 通配符，"" 表示场景为空）的资金净额：入账为正、出账为负 */
export interface SceneTerm { kind: 'scene', scene: string, sign: 1 | -1 }
/** 全部流水的资金净额 */
export interface FlowTerm { kind: 'flow', sign: 1 | -1 }
export type RuleTerm = ColumnTerm | SceneTerm | FlowTerm

/** 汇总项从第 3 行开始（0 起的行号 2） */
const FIRST_ITEM_ROW = 2

type RowKind = 'item' | 'total' | 'check-title' | 'check' | 'blank'

interface SummaryRowDef {
  label: string
  kind: RowKind
  /** 汇总项：由规则项构成 */
  terms?: RuleTerm[]
  /** 合计项：由其他行按正负号组合（公式引用单元格，下钻时展开为各行的规则项） */
  parts?: { label: string, sign: 1 | -1 }[]
  /** 公式外层套 ROUND(…,2) */
  round?: boolean
}

const col = (column: string, sign: 1 | -1 = 1, op?: 'negative' | 'positive'): RuleTerm => ({ kind: 'column', column, sign, ...(op ? { op } : {}) })
const scene = (pattern: string, sign: 1 | -1 = 1): RuleTerm => ({ kind: 'scene', scene: pattern, sign })
const sumOf = (labels: string[]) => labels.map(label => ({ label, sign: 1 as const }))

const INCOME = ['销售收入', '小额打款', '消费者赔付', '平台赔付', '平台补贴', '达人补贴', '抖音支付补贴', '抖音月付营销补贴', '旧主体转入', '银行补贴']
const EXPENSE = ['佣金', '佣金返还', '招商服务费', '站外推广费', '平台服务费', '运费险', '拦截费/上门取件', '货款充值保证金', '转入新主体', '千川投流']

/**
 * 第 3 行起的汇总项定义（顺序即表格行顺序），即「动账场景 / 金额字段 → 汇总项」的归类规则
 * 符号约定：收入区按资金影响（入账正、出账负）；支出区按成本（扣款正、退回负）
 */
const ROWS: SummaryRowDef[] = [
  // ── 收入 ──────────────────────────────────────────
  // 不含「运费实付」：该字段不计入动账金额
  { label: '销售收入', kind: 'item', terms: [col('订单实付应结'), col('订单退款')] },
  { label: '小额打款', kind: 'item', terms: [scene('小额打款'), scene('公益捐赠')] },
  { label: '消费者赔付', kind: 'item', terms: [scene('消费者赔付')] },
  { label: '平台赔付', kind: 'item', terms: [scene('平台赔付')] },
  { label: '平台补贴', kind: 'item', terms: ['实际平台补贴', '实际平台补贴_运费', '其他平台补贴', '以旧换新抵扣', '政府补贴平台垫资'].map(c => col(c)) },
  { label: '达人补贴', kind: 'item', terms: [col('实际达人补贴')] },
  { label: '抖音支付补贴', kind: 'item', terms: [col('实际抖音支付补贴')] },
  { label: '抖音月付营销补贴', kind: 'item', terms: [col('实际抖音月付营销补贴')] },
  { label: '旧主体转入', kind: 'item', terms: [scene('*转入*')] },
  { label: '银行补贴', kind: 'item', terms: [col('银行补贴')] },
  { label: '收入合计', kind: 'total', parts: sumOf(INCOME) },
  // ── 支出 ──────────────────────────────────────────
  { label: '佣金', kind: 'item', terms: [col('佣金', -1, 'negative'), col('服务商佣金', -1), col('渠道分成', -1), col('其他分成', -1)] },
  // 退款触发的佣金分账退回 + 达人未开票的佣金抵扣（场景为空的是同类冻结/撤销流水）
  { label: '佣金返还', kind: 'item', terms: [col('佣金', -1, 'positive'), scene('带货佣金未开票抵扣', -1), scene('', -1)] },
  { label: '招商服务费', kind: 'item', terms: [col('招商服务费', -1)] },
  { label: '站外推广费', kind: 'item', terms: [col('站外推广费', -1)] },
  { label: '平台服务费', kind: 'item', terms: [col('平台服务费', -1)] },
  { label: '运费险', kind: 'item', terms: [scene('退换货运费险', -1)] },
  { label: '拦截费/上门取件', kind: 'item', terms: [scene('快递拦截费', -1), scene('上门取件*', -1)] },
  { label: '货款充值保证金', kind: 'item', terms: [scene('*保证金*', -1)] },
  { label: '转入新主体', kind: 'item', terms: [scene('*转出*', -1)] },
  { label: '千川投流', kind: 'item', terms: [scene('*千川*', -1)] },
  { label: '支出合计', kind: 'total', parts: sumOf(EXPENSE) },
  // ── 结果 ──────────────────────────────────────────
  { label: '实际入账金额', kind: 'total', parts: [{ label: '收入合计', sign: 1 }, { label: '支出合计', sign: -1 }] },
  { label: '提现', kind: 'item', terms: [scene('提现', -1)] },
  { label: '余额变化', kind: 'total', parts: [{ label: '实际入账金额', sign: 1 }, { label: '提现', sign: -1 }] },
  { label: '', kind: 'blank' },
  // ── 核对区 ────────────────────────────────────────
  { label: '核对区', kind: 'check-title' },
  { label: '商户主体全称', kind: 'check' },
  { label: '流水净额(入账-出账)', kind: 'check', terms: [{ kind: 'flow', sign: 1 }] },
  // 为 0 说明每笔流水都恰好归入某个汇总项；不为 0 说明出现了新的动账场景，需要补充归类规则。
  // 下钻时 = 余额变化规则 − 全部流水：贡献不为 0 的流水就是没有被归类（或被重复归类）的流水
  { label: '未归类差额', kind: 'check', parts: [{ label: '余额变化', sign: 1 }, { label: '流水净额(入账-出账)', sign: -1 }], round: true },
]

const ROW_INDEX = new Map(ROWS.map((r, i) => [r.label, i]))

/** 汇总表中某行的行号（1 起） */
function rowNumber(label: string): number {
  const i = ROW_INDEX.get(label)
  if (i === undefined)
    throw new Error(`汇总表没有「${label}」行`)
  return FIRST_ITEM_ROW + i + 1
}

/** 展开为规则项（合计行递归展开），用于明细下钻；没有规则的行返回 null */
export function getDrillRule(label: string): RuleTerm[] | null {
  const row = ROWS.find(r => r.label === label)
  if (!row)
    return null
  if (row.terms)
    return row.terms
  if (row.parts) {
    return row.parts.flatMap(({ label: part, sign }) =>
      (getDrillRule(part) ?? []).map(t => ({ ...t, sign: (t.sign * sign) as 1 | -1 })))
  }
  return null
}

/** 汇总表数据行（0 起的行号 → 汇总项名称），供下钻识别选中的单元格 */
export function summaryLabelAt(row: number): string | null {
  const def = ROWS[row - FIRST_ITEM_ROW]
  return def && (def.terms || def.parts) ? def.label : null
}

interface FormulaContext {
  cell: (label: string) => string
  /** 按主体、店铺、月份对数据集的某个金额列求和（op 只取负数 / 正数部分） */
  sum: (term: ColumnTerm) => string
  /** 按主体、店铺、动账场景求资金净额：入账 − 出账 */
  net: (scene: string) => string
  /** 按主体、店铺求全部流水净额（不限场景；SUMIFS 的 "*" 不匹配空单元格，不能用来代替） */
  flow: () => string
}

function rowFormula(row: SummaryRowDef, c: FormulaContext): string | null {
  let expr: string
  if (row.terms) {
    expr = row.terms.map((t, i) => {
      const body = t.kind === 'column' ? c.sum(t) : t.kind === 'scene' ? c.net(t.scene) : c.flow()
      return t.sign < 0 ? `-${body}` : i === 0 ? body : `+${body}`
    }).join('')
  }
  else if (row.parts) {
    const rows = row.parts.map(p => rowNumber(p.label))
    const contiguous = row.parts.every(p => p.sign > 0) && rows.every((r, i) => i === 0 || r === (rows[i - 1] ?? 0) + 1)
    expr = contiguous && rows.length > 2
      ? `SUM(${c.cell(row.parts[0]!.label)}:${c.cell(row.parts[row.parts.length - 1]!.label)})`
      : row.parts.map((p, i) => `${p.sign < 0 ? '-' : i === 0 ? '' : '+'}${c.cell(p.label)}`).join('')
  }
  else {
    return null
  }
  return row.round ? `ROUND(${expr},2)` : expr
}

// ── 样式 ─────────────────────────────────────────────
const GREEN = '#C6EFCE'
const SUB_GREEN = '#A9DFB8'
const TOTAL_GREEN = '#8FD3A3'
const GRID = { s: 1, cl: { rgb: '#A9D8B4' } }
const GRID_BORDER = { t: GRID, b: GRID, l: GRID, r: GRID }
const FONT = { ff: 'Microsoft YaHei', fs: 11, cl: { rgb: '#1F1F1F' } }
const NUM_FMT = { pattern: '#,##0.00;-#,##0.00;"-"' }
const WRAP = 3 // WrapStrategy.WRAP

export const SUMMARY_STYLES = {
  'lg-period': { ...FONT, fs: 10, cl: { rgb: '#4B5563' }, bg: { rgb: GREEN }, ht: 1, vt: 1, bd: GRID_BORDER },
  'lg-entity': { ...FONT, fs: 12, bl: 1, bg: { rgb: GREEN }, ht: 2, vt: 2, bd: GRID_BORDER },
  'lg-shop': { ...FONT, bg: { rgb: GREEN }, ht: 2, vt: 2, tb: WRAP, bd: GRID_BORDER },
  'lg-corner': { ...FONT, bg: { rgb: GREEN }, bd: GRID_BORDER },
  'lg-label': { ...FONT, bg: { rgb: GREEN }, ht: 1, vt: 2, bd: GRID_BORDER },
  'lg-label-total': { ...FONT, bl: 1, bg: { rgb: GREEN }, ht: 1, vt: 2, bd: GRID_BORDER },
  'lg-num': { ...FONT, bg: { rgb: GREEN }, ht: 3, vt: 2, n: NUM_FMT, bd: GRID_BORDER },
  'lg-num-total': { ...FONT, bl: 1, bg: { rgb: GREEN }, ht: 3, vt: 2, n: NUM_FMT, bd: GRID_BORDER },
  'lg-check-title': { ...FONT, bl: 1, fs: 10, cl: { rgb: '#6B7280' }, ht: 1, vt: 2 },
  'lg-check-label': { ...FONT, fs: 10, cl: { rgb: '#6B7280' }, ht: 1, vt: 2 },
  'lg-check-text': { ...FONT, fs: 10, cl: { rgb: '#6B7280' }, ht: 2, vt: 2, tb: WRAP },
  'lg-check-num': { ...FONT, fs: 10, cl: { rgb: '#6B7280' }, ht: 3, vt: 2, n: NUM_FMT },
  // 主体合计 / 全部合计列：底色加深以区分
  'lg-sub-head': { ...FONT, bl: 1, bg: { rgb: SUB_GREEN }, ht: 2, vt: 2, tb: WRAP, bd: GRID_BORDER },
  'lg-sub-num': { ...FONT, bg: { rgb: SUB_GREEN }, ht: 3, vt: 2, n: NUM_FMT, bd: GRID_BORDER },
  'lg-sub-num-total': { ...FONT, bl: 1, bg: { rgb: SUB_GREEN }, ht: 3, vt: 2, n: NUM_FMT, bd: GRID_BORDER },
  'lg-total-head': { ...FONT, fs: 12, bl: 1, cl: { rgb: '#FFFFFF' }, bg: { rgb: '#107C41' }, ht: 2, vt: 2, bd: GRID_BORDER },
  'lg-total-num': { ...FONT, bl: 1, bg: { rgb: TOTAL_GREEN }, ht: 3, vt: 2, n: NUM_FMT, bd: GRID_BORDER },
}

/** t: 1 = 文本（防止 2026-10 之类的值被公式引擎当作日期） */
interface CellData { v?: string | number, f?: string, s?: string, t?: 1 }

/** 列序号（0 起）→ Excel 列字母 */
export function columnLetter(index: number): string {
  let n = index + 1
  let s = ''
  while (n > 0) {
    const m = (n - 1) % 26
    s = String.fromCharCode(65 + m) + s
    n = Math.floor((n - 1) / 26)
  }
  return s
}

function quoteText(v: string): string {
  return `"${v.replace(/"/g, '""')}"`
}

function createContext(colIndex: number): FormulaContext {
  const col = columnLetter(colIndex)
  const ds = quoteText(LEDGER_DATASET)
  // 主体全称（核对区）、店铺（第 2 行）、月份（A1）所在单元格，行号锁定
  const keys = `"商户主体名称",${col}$${SUMMARY_ROW.entityFull + 1},"店铺",${col}$${SUMMARY_ROW.shop + 1},"月份",$A$1`
  const sum = (t: ColumnTerm) => {
    const sign = t.op === 'negative' ? `,${quoteText(t.column)},"<0"` : t.op === 'positive' ? `,${quoteText(t.column)},">0"` : ''
    return `DSUMIFS(${ds},${quoteText(t.column)},${keys}${sign})`
  }
  // 资金净额 = 入账为正、出账为负（数据集计算列）
  const net = (scene: string) => `DSUMIFS(${ds},"资金净额",${keys},"动账场景",${quoteText(scene)})`
  const flow = () => `DSUMIFS(${ds},"资金净额",${keys})`
  return { cell: label => `${col}${rowNumber(label)}`, sum, net, flow }
}

/** 默认主体简称：去掉公司类型和常见行业后缀、城市前缀，如「上海某某电子商务服务有限公司」→「某某电子」 */
export function shortEntityName(full: string): string {
  let s = full.replace(/(股份)?有限(责任)?公司$/, '')
  s = s.replace(/(健康科技|网络科技|信息技术|科技|商务服务|电子商务|贸易|实业|发展|服务)$/, '')
  const city = s.match(/^(北京|上海|天津|重庆|广州|深圳|杭州|南京|苏州|成都|武汉|长沙|西安|郑州|合肥|福州|厦门|济南|青岛|宁波|东莞|佛山|昆明|南昌|沈阳|大连|哈尔滨|长春|石家庄|太原|南宁|贵阳|海口|兰州|银川|西宁|乌鲁木齐|呼和浩特|拉萨)/)
  if (city && s.length - city[0].length >= 3)
    s = s.slice(city[0].length)
  return s || full
}

export interface SheetLayout {
  cellData: Record<number, Record<number, CellData>>
  mergeData: { startRow: number, endRow: number, startColumn: number, endColumn: number }[]
  columnData: Record<number, { w: number }>
  rowData: Record<number, { h: number, hd: 0 }>
  rowCount: number
  columnCount: number
}

/** 合计列在第 2 行的标识（也是下钻识别依据，后端禁止用作店铺名） */
export const SUBTOTAL_SHOP_LABEL = '主体合计'
export const TOTAL_SHOP_LABEL = '全部店铺'
/** 全部合计列在核对区「商户主体全称」行的取值 */
export const TOTAL_ENTITY_LABEL = '全部主体'

type LayoutColumn = { kind: 'shop', column: SummaryColumn } | { kind: 'subtotal', entityFull: string, shopCols: number[] } | { kind: 'total', shopCols: number[] }

/** 列布局：同主体店铺相邻，≥2 个店铺时追加「主体合计」；店铺列 ≥2 时最右追加「全部合计」 */
function planColumns(columns: SummaryColumn[]): LayoutColumn[] {
  const layout: LayoutColumn[] = []
  const allShopCols: number[] = []
  let i = 0
  while (i < columns.length) {
    const entity = columns[i]!.entityFull
    const groupCols: number[] = []
    while (i < columns.length && columns[i]!.entityFull === entity) {
      layout.push({ kind: 'shop', column: columns[i]! })
      groupCols.push(layout.length) // 汇总表列号 = layout 下标 + 1
      i++
    }
    allShopCols.push(...groupCols)
    if (groupCols.length > 1)
      layout.push({ kind: 'subtotal', entityFull: entity, shopCols: groupCols })
  }
  if (allShopCols.length > 1)
    layout.push({ kind: 'total', shopCols: allShopCols })
  return layout
}

/** 合计列公式：连续列用 SUM(B3:C3)，不连续用 SUM(B3,C3,E3) */
function sumOfColumns(cols: number[], row: number): string {
  const contiguous = cols.every((c, i) => i === 0 || c === (cols[i - 1] ?? 0) + 1)
  const refs = cols.map(c => `${columnLetter(c)}${row}`)
  return contiguous && cols.length > 1 ? `SUM(${refs[0]}:${refs[refs.length - 1]})` : `SUM(${refs.join(',')})`
}

/**
 * @param columns 已按主体分组排好序（同主体的店铺相邻）
 * @param period 月份 YYYY-MM，写入 A1，全部公式按 A1 取数
 */
export function buildSummarySheet(columns: SummaryColumn[], period: string): SheetLayout {
  const cellData: SheetLayout['cellData'] = {}
  const setCell = (r: number, c: number, cell: CellData) => {
    cellData[r] ??= {}
    cellData[r][c] = cell
  }

  setCell(SUMMARY_ROW.entity, 0, { v: period, t: 1, s: 'lg-period' })
  setCell(SUMMARY_ROW.shop, 0, { s: 'lg-corner' })
  ROWS.forEach((row, i) => {
    const style = {
      'item': 'lg-label',
      'total': 'lg-label-total',
      'check-title': 'lg-check-title',
      'check': 'lg-check-label',
      'blank': undefined,
    }[row.kind]
    if (row.label)
      setCell(FIRST_ITEM_ROW + i, 0, { v: row.label, s: style })
  })

  const layout = planColumns(columns)
  layout.forEach((col, i) => {
    const c = i + 1
    const prev = layout[i - 1]
    const entityOf = (x: LayoutColumn | undefined) => x?.kind === 'shop' ? x.column.entityFull : x?.kind === 'subtotal' ? x.entityFull : null

    if (col.kind === 'shop') {
      const ctx = createContext(c)
      // 同主体的列会合并：简称只写在组内首列，被合并的单元格留空，避免残留旧值
      const groupStart = entityOf(prev) !== col.column.entityFull
      setCell(SUMMARY_ROW.entity, c, groupStart ? { v: col.column.entityShort, s: 'lg-entity' } : { s: 'lg-entity' })
      setCell(SUMMARY_ROW.shop, c, { v: col.column.shop, t: 1, s: 'lg-shop' })
      ROWS.forEach((row, ri) => {
        const r = FIRST_ITEM_ROW + ri
        if (r === SUMMARY_ROW.entityFull) {
          setCell(r, c, { v: col.column.entityFull, t: 1, s: 'lg-check-text' })
          return
        }
        const formula = rowFormula(row, ctx)
        if (formula)
          setCell(r, c, { f: `=${formula}`, s: row.kind === 'total' ? 'lg-num-total' : row.kind === 'check' ? 'lg-check-num' : 'lg-num' })
      })
      return
    }

    // 合计列：对店铺列求和
    const isTotal = col.kind === 'total'
    setCell(SUMMARY_ROW.entity, c, isTotal ? { v: '合计', s: 'lg-total-head' } : { s: 'lg-entity' })
    setCell(SUMMARY_ROW.shop, c, { v: isTotal ? TOTAL_SHOP_LABEL : SUBTOTAL_SHOP_LABEL, s: isTotal ? 'lg-total-head' : 'lg-sub-head' })
    ROWS.forEach((row, ri) => {
      const r = FIRST_ITEM_ROW + ri
      if (r === SUMMARY_ROW.entityFull) {
        setCell(r, c, { v: isTotal ? TOTAL_ENTITY_LABEL : col.entityFull, s: 'lg-check-text' })
        return
      }
      if (!row.terms && !row.parts)
        return
      const style = row.kind === 'check' ? 'lg-check-num' : isTotal ? 'lg-total-num' : row.kind === 'total' ? 'lg-sub-num-total' : 'lg-sub-num'
      const formula = sumOfColumns(col.shopCols, r + 1)
      setCell(r, c, { f: `=${row.round ? `ROUND(${formula},2)` : formula}`, s: style })
    })
  })

  // 第 1 行：同一主体的店铺列与主体合计列合并
  const mergeData: SheetLayout['mergeData'] = []
  let start = 0
  const entityAt = (i: number) => {
    const x = layout[i]
    return x?.kind === 'shop' ? x.column.entityFull : x?.kind === 'subtotal' ? x.entityFull : null
  }
  for (let i = 1; i <= layout.length; i++) {
    if (i === layout.length || entityAt(i) !== entityAt(start) || entityAt(i) === null) {
      if (i - start > 1 && entityAt(start) !== null)
        mergeData.push({ startRow: 0, endRow: 0, startColumn: start + 1, endColumn: i })
      start = i
    }
  }

  const columnData: SheetLayout['columnData'] = { 0: { w: 170 } }
  layout.forEach((col, i) => {
    columnData[i + 1] = { w: col.kind === 'shop' ? 150 : 140 }
  })

  const rowData: SheetLayout['rowData'] = {
    [SUMMARY_ROW.entity]: { h: 56, hd: 0 },
    [SUMMARY_ROW.shop]: { h: 56, hd: 0 },
  }
  for (let r = FIRST_ITEM_ROW; r < FIRST_ITEM_ROW + ROWS.length; r++)
    rowData[r] = { h: 28, hd: 0 }
  rowData[SUMMARY_ROW.entityFull] = { h: 40, hd: 0 }

  return {
    cellData,
    mergeData,
    columnData,
    rowData,
    rowCount: Math.max(FIRST_ITEM_ROW + ROWS.length + 10, 50),
    columnCount: Math.max(layout.length + 4, 12),
  }
}

/** 从已有汇总表读取 主体全称 → 简称（保留用户手动修改过的简称） */
export function readEntityAliases(cellData: Record<number, Record<number, { v?: unknown } | null | undefined>> | undefined): Map<string, string> {
  const aliases = new Map<string, string>()
  if (!cellData)
    return aliases
  const entityRow = cellData[SUMMARY_ROW.entity] ?? {}
  const fullRow = cellData[SUMMARY_ROW.entityFull] ?? {}
  const shopRow = cellData[SUMMARY_ROW.shop] ?? {}
  for (let c = 1; fullRow[c]?.v != null && fullRow[c]?.v !== ''; c++) {
    const full = String(fullRow[c]?.v)
    const short = entityRow[c]?.v
    // 合计列不是主体；第 1 行是合并单元格，以每组首列（第一次出现）的值为准
    if (full === TOTAL_ENTITY_LABEL || shopRow[c]?.v === SUBTOTAL_SHOP_LABEL)
      continue
    if (!aliases.has(full) && short != null && short !== '')
      aliases.set(full, String(short))
  }
  return aliases
}
