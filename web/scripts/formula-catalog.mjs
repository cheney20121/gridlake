#!/usr/bin/env node
/**
 * Excel 函数兼容清单：`pnpm formula:catalog`（重新生成）/ `pnpm formula:catalog --check`（CI：检查生成物是最新的）
 *
 * 从表格内核（Univer 的 @univerjs/engine-formula）读出：
 *  - 函数目录：按 Excel 的类别列出的全部函数名（FUNCTION_NAMES_* 枚举）
 *  - 已实现的函数：各类别注册数组（functionMath = [[Abs, FUNCTION_NAMES_MATH.ABS], …]）里的函数
 * 再加上 Excel 有、内核目录里还没有的新函数（formula-compat.ts 的 _xlfn. 前缀表里能找到的，以及下面 EXTRA 里列的），
 * 生成两个文件：
 *  - docs/excel-functions.md：给人看的兼容清单
 *  - src/features/excel/formula/function-catalog.ts：运行时用（导入 xlsx、公式诊断时点名不支持的函数）
 *
 * 升级 Univer 后重新生成即可；内核新出现了不认识的类别或函数时脚本会报错，请补上下面的中文说明
 */
import fs from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const DOC = path.resolve(ROOT, '../docs/excel-functions.md')
const TS = path.join(ROOT, 'src/features/excel/formula/function-catalog.ts')
const check = process.argv.includes('--check')

// pnpm 的依赖是严格隔离的：公式引擎是预设包的依赖，和它放在同一个 node_modules/@univerjs 下
const preset = fs.realpathSync(path.join(ROOT, 'node_modules/@univerjs/preset-sheets-core'))
const engineDir = path.join(path.dirname(preset), 'engine-formula')
const engineVersion = JSON.parse(fs.readFileSync(path.join(engineDir, 'package.json'), 'utf8')).version
const engine = fs.readFileSync(path.join(engineDir, 'lib/es/index.js'), 'utf8')

const CATEGORY = {
  MATH: '数学与三角',
  STATISTICAL: '统计',
  LOOKUP: '查找与引用',
  TEXT: '文本',
  LOGICAL: '逻辑',
  DATE: '日期与时间',
  INFORMATION: '信息',
  FINANCIAL: '财务',
  ENGINEERING: '工程',
  DATABASE: '数据库',
  COMPATIBILITY: '兼容性（旧版函数）',
  CUBE: '多维数据集',
  WEB: 'Web',
}
/** 内核里的非 Excel 类别：ARRAY 是 Google 表格的函数，UNIVER 为空 */
const NOT_EXCEL = { ARRAY: 'Google 表格函数（额外支持）', UNIVER: '' }

/** Excel 有、内核目录里还没有的函数（类别） */
const EXTRA = {
  PIVOTBY: 'LOOKUP',
  TRIMRANGE: 'LOOKUP',
  TRANSLATE: 'TEXT',
  DETECTLANGUAGE: 'TEXT',
  COPILOT: 'TEXT',
}

/** 不支持的原因 */
const REASON = {
  'CUBE': '需要连接 OLAP / Power Pivot 数据模型；大数据请用数据集与透视表',
  'RTD': '需要 Windows 上的实时数据服务器（COM）',
  'CALL': '调用本机程序库，网页里不能运行',
  'REGISTER.ID': '调用本机程序库，网页里不能运行',
  'WEBSERVICE': '浏览器里不能任意访问外部网址；跨工作簿取数用 IMPORTRANGE，接口数据用「数据连接」',
  'EUROCONVERT': '依赖 Excel 的欧元转换加载项',
  'PHONETIC': '读取日文注音（ふりがな）信息，表格里不保存',
  'INFO': '返回操作系统、内存等本机环境信息',
  'GETPIVOTDATA': 'Gridlake 的透视表是服务端计算后写成的普通工作表，直接引用单元格，或用 DSUMIFS 按条件汇总',
  'COPILOT': '需要微软的 AI 服务；可用右上角的 AI 助手',
  'TRANSLATE': '需要微软的翻译服务',
  'DETECTLANGUAGE': '需要微软的翻译服务',
  'PIVOTBY': '内核尚未实现；可用「透视分析」或 =SQL("… GROUP BY …")',
}
const DEFAULT_REASON = '表格内核尚未实现'

/** Gridlake 平台函数（Excel 里没有；导出 xlsx 时写成计算结果，原公式记在批注里） */
const PLATFORM = [
  ['DSUMIFS', '对数据集（服务端大数据表）按条件求和'],
  ['DCOUNTIFS', '对数据集按条件计数'],
  ['DAVERAGEIFS', '对数据集按条件求平均'],
  ['DMAXIFS', '对数据集按条件取最大值'],
  ['DMINIFS', '对数据集按条件取最小值'],
  ['DCOUNTDISTINCTIFS', '对数据集按条件统计不重复的取值个数'],
  ['DLOOKUP', '在数据集中查找，返回第一条匹配行的值'],
  ['TSUM', '对工作表里的数据表求和（按它当前的筛选）'],
  ['TCOUNT', '数据表的行数 / 按条件计数'],
  ['TAVERAGE', '数据表某列的平均值'],
  ['TMIN', '数据表某列的最小值'],
  ['TMAX', '数据表某列的最大值'],
  ['TVALUE', '数据表当前排序下第 N 行的值'],
  ['SQL', '在服务端执行只读 SQL，结果带表头铺开'],
  ['SQLVALUE', '执行只读 SQL，取第一行第一列'],
  ['IMPORTRANGE', '读取另一个工作簿的区域'],
]

// ── 读内核 ──
const catalog = {}
for (const m of engine.matchAll(/let (FUNCTION_NAMES_([A-Z]+)) = \/\* @__PURE__ \*\/ function\(\1\) \{([\s\S]*?)return \1;/g))
  catalog[m[2]] = [...m[3].matchAll(/\["([\w.]+)"\] = "([^"]+)"/g)].map(x => ({ key: x[1], name: x[2] }))
if (!catalog.MATH?.length)
  throw new Error('没有从公式引擎里读到函数目录（Univer 的打包方式变了？）')
const keyToName = new Map(Object.entries(catalog).flatMap(([cat, list]) => list.map(f => [`${cat}.${f.key}`, f.name])))
const implemented = new Set()
for (const m of engine.matchAll(/const function[A-Z][A-Za-z]* = \[([\s\S]*?)\];\n/g)) {
  for (const x of m[1].matchAll(/FUNCTION_NAMES_([A-Z]+)\.([A-Z0-9_]+)\]|,\s*"([A-Z0-9_.]+)"\]/g))
    implemented.add(x[3] ?? keyToName.get(`${x[1]}.${x[2]}`) ?? x[2])
}
for (const cat of Object.keys(catalog)) {
  if (!(cat in CATEGORY) && !(cat in NOT_EXCEL))
    throw new Error(`公式引擎里有新的函数类别「${cat}」：请在 CATEGORY 里补上中文名`)
}

// 导出时需要前缀的函数（formula-compat.ts 里的表）
const compat = fs.readFileSync(path.join(ROOT, 'src/features/excel/xlsx/formula-compat.ts'), 'utf8')
const future = new Set(/const FUTURE = new Set\(`([^`]*)`/.exec(compat)?.[1].split(/\s+/).filter(Boolean) ?? [])
const worksheet = new Set(/const WORKSHEET = new Set\(\[([^\]]*)\]\)/.exec(compat)?.[1].match(/'[^']+'/g)?.map(s => s.slice(1, -1)) ?? [])
if (!future.size)
  throw new Error('没有从 formula-compat.ts 读到 FUTURE 表')

const inCatalog = new Set(Object.values(catalog).flat().map(f => f.name))
for (const f of future) {
  if (!inCatalog.has(f) && !(f in EXTRA))
    throw new Error(`Excel 新函数 ${f} 不在内核目录里：请在 EXTRA 里注明它的类别`)
}

/** 类别 → 函数（Excel 的函数，含内核目录外的新函数） */
const excel = new Map(Object.keys(CATEGORY).map(cat => [cat, [...new Set((catalog[cat] ?? []).map(f => f.name))]]))
for (const [name, cat] of Object.entries(EXTRA)) {
  if (!excel.get(cat).includes(name))
    excel.get(cat).push(name)
}
for (const list of excel.values())
  list.sort()
const all = [...excel.values()].flat()
const supported = all.filter(f => implemented.has(f))
const unsupported = all.filter(f => !implemented.has(f))
const extraSupported = Object.entries(NOT_EXCEL).flatMap(([cat]) => (catalog[cat] ?? []).map(f => f.name)).filter(f => implemented.has(f)).sort()
const categoryOf = name => [...excel.entries()].find(([, l]) => l.includes(name))?.[0]

// ── 文档 ──
const code = list => list.map(f => `\`${f}\``).join(' ')
const pct = Math.round((supported.length / all.length) * 1000) / 10
const lines = [
  '# Excel 函数兼容清单',
  '',
  `> 由 \`web/scripts/formula-catalog.mjs\` 从表格内核（Univer \`@univerjs/engine-formula\` ${engineVersion}）的函数目录生成，不要手改；CI 会检查它是最新的（\`pnpm formula:catalog --check\`）。`,
  '',
  `**按表格内核整理的 Excel 函数目录共 ${all.length} 个（含少数 Google 表格的同名扩展），支持 ${supported.length} 个（${pct}%）**，包括 \`XLOOKUP\`、\`FILTER\`、\`UNIQUE\`、\`SORTBY\`、\`LET\`、\`LAMBDA\`、\`MAP\`、\`VSTACK\`、\`TEXTSPLIT\`、\`REGEXTEST\`、\`GROUPBY\` 这些较新的动态数组函数。另有 ${PLATFORM.length} 个 Gridlake 平台函数（Excel 里没有）。`,
  '',
  '- 公式在浏览器里计算；引用大数据集的计算请用平台函数（`DSUMIFS`、`SQL` 等），在服务端算，见 [formulas.md](formulas.md)。',
  `- 导入 xlsx：文件里的 \`_xlfn.\` / \`_xlws.\` / \`_xlpm.\` 前缀自动去掉；用到不支持的函数时，导入结果里会点名是哪几个函数（这些公式显示 \`#NAME?\`）。`,
  `- 导出 xlsx：Excel 2010 之后新增的 ${future.size} 个函数自动加 \`_xlfn.\` 前缀（\`FILTER\`、\`SORT\` 为 \`_xlfn._xlws.\`），旧版 Excel 打开时能识别；平台函数写成计算结果，原公式记在批注里。`,
  '',
  `## 不支持的函数（${unsupported.length}）`,
  '',
  '| 函数 | 类别 | 原因 / 替代 |',
  '|---|---|---|',
  ...unsupported.map(f => `| \`${f}\` | ${CATEGORY[categoryOf(f)]} | ${REASON[f] ?? REASON[categoryOf(f)] ?? DEFAULT_REASON} |`),
  '',
  `## 平台函数（${PLATFORM.length}）`,
  '',
  '| 函数 | 说明 |',
  '|---|---|',
  ...PLATFORM.map(([f, d]) => `| \`${f}\` | ${d} |`),
  '',
  '## 按类别',
  '',
  '标 <sup>新</sup> 的是 Excel 2010 之后新增的函数（导出时加前缀）。',
  '',
]
for (const [cat, list] of excel) {
  const ok = list.filter(f => implemented.has(f))
  const no = list.filter(f => !implemented.has(f))
  lines.push(`### ${CATEGORY[cat]}（支持 ${ok.length} / ${list.length}）`, '')
  if (ok.length)
    lines.push(ok.map(f => `\`${f}\`${future.has(f) || worksheet.has(f) ? '<sup>新</sup>' : ''}`).join(' '), '')
  if (no.length)
    lines.push(`不支持：${code(no)}`, '')
}
if (extraSupported.length)
  lines.push(`### ${NOT_EXCEL.ARRAY}`, '', `Excel 里没有、Google 表格里有的函数，这里也能用（导出 xlsx 后 Excel 不认识）：${code(extraSupported)}`, '')
const doc = `${lines.join('\n').trimEnd()}\n`

// ── 运行时用的名单 ──
/** 生成的 TS 里的字符串：单引号（与仓库的代码风格一致） */
const sq = s => `'${s.replace(/\\/g, '\\\\').replace(/'/g, '\\\'')}'`
const wrap = list => list.join(' ').replace(/(.{1,120})(\s|$)/g, '$1\n').trimEnd().split('\n').map(l => `  ${l}`).join('\n')
const ts = `// 由 scripts/formula-catalog.mjs 生成，不要手改（pnpm formula:catalog 重新生成；CI 检查是最新的）
// 表格内核：@univerjs/engine-formula ${engineVersion}

/** 表格内核实现了的函数（含 Google 表格扩展函数） */
export const BUILTIN_FUNCTIONS: ReadonlySet<string> = new Set(\`
${wrap([...supported, ...extraSupported].sort())}
\`.split(/\\s+/).filter(Boolean))

/** Excel 有、这里不支持的函数 → 原因 / 替代 */
export const UNSUPPORTED_FUNCTIONS: Readonly<Record<string, string>> = {
${unsupported.map(f => `  ${sq(f)}: ${sq(REASON[f] ?? REASON[categoryOf(f)] ?? DEFAULT_REASON)},`).join('\n')}
}

/** Gridlake 平台函数（Excel 里没有） */
export const PLATFORM_FUNCTIONS: ReadonlySet<string> = new Set([${PLATFORM.map(([f]) => sq(f)).join(', ')}])
`

const outputs = [[DOC, doc], [TS, ts]]
if (check) {
  const stale = outputs.filter(([p, text]) => !fs.existsSync(p) || fs.readFileSync(p, 'utf8') !== text).map(([p]) => path.relative(path.resolve(ROOT, '..'), p))
  if (stale.length) {
    console.error(`函数兼容清单不是最新的：${stale.join('、')}\n请运行 pnpm formula:catalog 重新生成并提交`)
    process.exit(1)
  }
  console.log(`函数兼容清单是最新的（Excel 函数 ${all.length} 个，支持 ${supported.length} 个）`)
}
else {
  for (const [p, text] of outputs)
    fs.writeFileSync(p, text)
  console.log(`已生成 ${path.relative(path.resolve(ROOT, '..'), DOC)} 和 ${path.relative(path.resolve(ROOT, '..'), TS)}：Excel 函数 ${all.length} 个，支持 ${supported.length} 个，不支持 ${unsupported.length} 个`)
}
