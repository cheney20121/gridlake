import { BUILTIN_FUNCTIONS, PLATFORM_FUNCTIONS, UNSUPPORTED_FUNCTIONS } from './function-catalog'

/**
 * 公式里用到的、这里算不了的函数：导入 xlsx 后、公式诊断时点名（这些公式显示 #NAME?）
 *
 * - unsupported：Excel 有、这里不支持的函数（附原因 / 替代），见 docs/excel-functions.md
 * - unknown：既不是 Excel 函数、也不是平台函数的名字（多半是拼错了，或者别的软件的自定义函数）
 */
export interface FunctionProblems {
  unsupported: { name: string, reason: string }[]
  unknown: string[]
}

/** 公式里调用的函数名（大写、去掉 _xlfn. 等前缀；字符串里的不算） */
export function calledFunctions(formula: string): string[] {
  const code = formula.replace(/"(?:[^"]|"")*"/g, '""')
  const names = new Set<string>()
  for (const m of code.matchAll(/(?<![\w.])((?:_xl(?:fn|ws)\.)*)([A-Z_][\w.]*)\s*\(/gi))
    names.add(m[2]!.toUpperCase())
  return [...names]
}

/** 名字是 LET / LAMBDA 里定义的变量（=LET(f, LAMBDA(x, x+1), f(2)) 里的 f）：不算未知函数 */
function letNames(formula: string): Set<string> {
  const out = new Set<string>()
  for (const m of formula.matchAll(/\bLET\s*\(([^()]*)/gi)) {
    const parts = m[1]!.split(',')
    for (let i = 0; i < parts.length - 1; i += 2)
      out.add(parts[i]!.trim().toUpperCase())
  }
  return out
}

export function functionProblems(formula: string): FunctionProblems {
  const local = letNames(formula)
  const unsupported: FunctionProblems['unsupported'] = []
  const unknown: string[] = []
  for (const name of calledFunctions(formula)) {
    if (BUILTIN_FUNCTIONS.has(name) || PLATFORM_FUNCTIONS.has(name) || local.has(name))
      continue
    const reason = UNSUPPORTED_FUNCTIONS[name]
    if (reason)
      unsupported.push({ name, reason })
    else
      unknown.push(name)
  }
  return { unsupported, unknown }
}
