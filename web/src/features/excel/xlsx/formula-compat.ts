import { rewriteCalls } from '@/features/excel/datatable/formula-syntax'

/**
 * Excel 文件里的公式写法与表格里的写法互转
 *
 * Excel 2010 之后新增的函数在文件里带前缀（老版本据此识别「不认识的函数」）：
 * - `_xlfn.XLOOKUP(...)`、`_xlfn.IFS(...)`；FILTER / SORT 为 `_xlfn._xlws.FILTER(...)`
 * - LET / LAMBDA 的参数名带 `_xlpm.`：`_xlfn.LET(_xlpm.x, 1, _xlpm.x + 1)`
 * - `@` 隐式交集写成 `_xlfn.SINGLE(...)`
 * 导入时去掉前缀，导出时加回去；不加的话 Excel 打开显示 #NAME?
 */

/** 需要 _xlfn. 前缀的函数（Excel 2010 及之后新增） */
const FUTURE = new Set(`ACOT ACOTH AGGREGATE ARABIC ARRAYTOTEXT BASE BETA.DIST BETA.INV BINOM.DIST BINOM.DIST.RANGE BINOM.INV BITAND BITLSHIFT BITOR BITRSHIFT BITXOR
BYCOL BYROW CEILING.MATH CEILING.PRECISE CHISQ.DIST CHISQ.DIST.RT CHISQ.INV CHISQ.INV.RT CHISQ.TEST CHOOSECOLS CHOOSEROWS COMBINA CONCAT CONFIDENCE.NORM
CONFIDENCE.T COT COTH COVARIANCE.P COVARIANCE.S CSC CSCH DAYS DECIMAL DROP ENCODEURL ERF.PRECISE ERFC.PRECISE EXPAND EXPON.DIST F.DIST F.DIST.RT F.INV
F.INV.RT F.TEST FILTERXML FLOOR.MATH FLOOR.PRECISE FORECAST.ETS FORECAST.ETS.CONFINT FORECAST.ETS.SEASONALITY FORECAST.ETS.STAT FORECAST.LINEAR FORMULATEXT
GAMMA GAMMA.DIST GAMMA.INV GAMMALN.PRECISE GAUSS GROUPBY HSTACK HYPGEOM.DIST IFNA IFS IMAGE IMCOSH IMCOT IMCSC IMCSCH IMSEC IMSECH IMSINH IMTAN ISFORMULA
ISOMITTED ISOWEEKNUM LAMBDA LET LOGNORM.DIST LOGNORM.INV MAKEARRAY MAP MAXIFS MINIFS MODE.MULT MODE.SNGL MUNIT NEGBINOM.DIST NETWORKDAYS.INTL NORM.DIST
NORM.INV NORM.S.DIST NORM.S.INV NUMBERVALUE PDURATION PERCENTILE.EXC PERCENTILE.INC PERCENTOF PERCENTRANK.EXC PERCENTRANK.INC PERMUTATIONA PHI PIVOTBY
POISSON.DIST QUARTILE.EXC QUARTILE.INC RANDARRAY RANK.AVG RANK.EQ REDUCE REGEXEXTRACT REGEXREPLACE REGEXTEST RRI SCAN SEC SECH SEQUENCE SHEET SHEETS
SKEW.P SORTBY STDEV.P STDEV.S SWITCH T.DIST T.DIST.2T T.DIST.RT T.INV T.INV.2T T.TEST TAKE TEXTAFTER TEXTBEFORE TEXTJOIN TEXTSPLIT TOCOL TOROW TRIMRANGE
UNICHAR UNICODE UNIQUE VALUETOTEXT VAR.P VAR.S VSTACK WEBSERVICE WEIBULL.DIST WORKDAY.INTL WRAPCOLS WRAPROWS XLOOKUP XMATCH XOR Z.TEST`.split(/\s+/))

/** 需要 _xlfn._xlws. 前缀的函数 */
const WORKSHEET = new Set(['FILTER', 'SORT'])

const PREFIX = /_xl(?:fn|ws|pm)\./i

/** 只在字符串字面量之外替换 */
function replaceOutsideStrings(text: string, re: RegExp, repl: (m: string, ...g: string[]) => string): string {
  return text.split(/("(?:[^"]|"")*")/).map((part, i) => (i % 2 === 1 ? part : part.replace(re, repl as never))).join('')
}

/** 文件中的公式（不含 =）→ 表格里的写法 */
export function fromExcelFormula(formula: string): string {
  if (!PREFIX.test(formula))
    return formula
  let out = rewriteCalls(formula, (name, args) => {
    const bare = name.replace(/^(?:_xlfn\.)?(?:_xlws\.)?/i, '')
    // @ 隐式交集：表格里按单值计算，直接取参数
    if (/^SINGLE$/i.test(bare) && /^_xlfn\./i.test(name) && args.length === 1)
      return args[0]!.trim()
    return bare === name ? null : `${bare}(${args.join(',')})`
  })
  out = replaceOutsideStrings(out, /_xlpm\./gi, () => '')
  return out
}

/** 一段文本里把这些名称（LET / LAMBDA 的参数）加上 _xlpm. 前缀 */
function prefixNames(text: string, names: string[]): string {
  if (!names.length)
    return text
  const alt = names.map(n => n.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|')
  // 前后不能是名称字符或 .，后面不能紧跟左括号（同名函数）或 !（工作表名）
  const re = new RegExp(`(^|[^\\w.\\u4E00-\\u9FFF])(${alt})(?![\\w.\\u4E00-\\u9FFF!(])`, 'gi')
  return replaceOutsideStrings(text, re, (_m, pre, name) => `${pre}_xlpm.${name}`)
}

/** 表格里的公式（不含 =）→ 文件中的写法 */
export function toExcelCompatFormula(formula: string): string {
  return rewriteCalls(formula, (name, args) => {
    const upper = name.toUpperCase()
    if (upper.startsWith('_XL'))
      return null
    if (WORKSHEET.has(upper))
      return `_xlfn._xlws.${name}(${args.join(',')})`
    if (!FUTURE.has(upper))
      return null
    if ((upper === 'LET' || upper === 'LAMBDA') && args.length >= 2) {
      // LET(名称1, 值1, 名称2, 值2, …, 计算)；LAMBDA(参数1, 参数2, …, 计算)
      const isName = (i: number) => (upper === 'LET' ? i % 2 === 0 && i < args.length - 1 : i < args.length - 1)
      const names = args.filter((_, i) => isName(i)).map(a => a.trim()).filter(n => /^[A-Z_\u4E00-\u9FFF][\w.\u4E00-\u9FFF]*$/i.test(n))
      const out = args.map((a, i) => (isName(i) ? a.replace(a.trim(), `_xlpm.${a.trim()}`) : prefixNames(a, names)))
      return `_xlfn.${name}(${out.join(',')})`
    }
    return `_xlfn.${name}(${args.join(',')})`
  })
}
