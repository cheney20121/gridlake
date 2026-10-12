/** 行数的简短写法：1,234 / 1.2 万 / 20 万 / 9,999 万 / 1 亿 / 1.2 亿（侧栏等放不下完整数字的地方） */
export function formatRowCount(n: number): string {
  const trim = (v: number, digits: number) => String(Number(v.toFixed(digits)))
  if (n >= 100_000_000)
    return `${trim(n / 100_000_000, n >= 10_000_000_000 ? 0 : 1)}亿`
  if (n >= 100_000)
    return `${Math.round(n / 10_000).toLocaleString('zh-CN')}万`
  if (n >= 10_000)
    return `${trim(n / 10_000, 1)}万`
  return n.toLocaleString('zh-CN')
}
