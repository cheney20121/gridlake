/**
 * 汉字拼音首字母（命令面板用，如「数据集」→ sjj）。不引入拼音词库：浏览器自带的中文排序规则（拼音序）
 * 就能判断一个汉字落在哪个字母区间，二十三个区间的起始字作为分界。
 * 多音字取排序规则给出的读音，常见多音字（行、重、长…）补充其他读音并一起匹配，所以一个字可能对应多个首字母
 */

/**
 * 各字母区间在排序规则里的第一个字（没有 i、u、v 开头的字）。逐字对照一份独立的拼音库校验过：
 * 常用字区（U+4E00–9FA5）两万多个字里只有「呒」「鸀」两个生僻字不对
 */
const BOUNDS = '吖丷嚓咑妸发旮過丌咔垃呣拏喔妑七呥仨他屲夕丫帀'
const LETTERS = 'abcdefghjklmnopqrstwxyz'

/** 常见多音字的全部首字母（与排序规则给出的读音取并集） */
const POLYPHONES: Record<string, string> = {
  行: 'hx',
  重: 'cz',
  长: 'cz',
  率: 'ls',
  调: 'dt',
  会: 'hk',
  乐: 'ly',
  藏: 'cz',
  降: 'jx',
  合: 'gh',
  传: 'cz',
  单: 'ds',
  查: 'cz',
  参: 'cs',
  卡: 'kq',
  曾: 'cz',
  朝: 'cz',
  省: 'sx',
}

let collator: Intl.Collator | null | undefined

function getCollator() {
  if (collator === undefined) {
    try {
      collator = new Intl.Collator('zh-Hans-CN-u-co-pinyin')
      // 运行环境没有中文排序规则时（精简版 ICU）排序结果与字符顺序一致，无法使用
      if (collator.compare('阿', '座') >= 0 || collator.compare('八', '啊') <= 0)
        collator = null
    }
    catch {
      collator = null
    }
  }
  return collator
}

const cache = new Map<string, string>()

/** 一个汉字的首字母（排序规则读音）；不是汉字或无法判断时返回空串 */
function primaryInitial(ch: string): string {
  const c = getCollator()
  if (!c || ch < '一' || ch > '龥')
    return ''
  let lo = 0
  let hi = BOUNDS.length - 1
  if (c.compare(ch, BOUNDS[0]!) < 0)
    return 'a'
  // 找最后一个不大于该字的分界
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1
    if (c.compare(BOUNDS[mid]!, ch) <= 0)
      lo = mid
    else
      hi = mid - 1
  }
  return LETTERS[lo]!
}

/**
 * 文本逐字的可能首字母：汉字为一个或多个字母，字母数字取自身（小写），其他符号（空格、标点）跳过
 */
export function initialsOf(text: string): string[][] {
  const out: string[][] = []
  for (const ch of text) {
    if (/[a-z0-9]/i.test(ch)) {
      out.push([ch.toLowerCase()])
      continue
    }
    const key = cache.get(ch)
    let letters: string
    if (key !== undefined) {
      letters = key
    }
    else {
      const p = primaryInitial(ch)
      const more = POLYPHONES[ch]
      letters = p && more ? [...new Set(p + more)].join('') : p
      cache.set(ch, letters)
    }
    if (letters)
      out.push([...letters])
  }
  return out
}

/** 查询串（字母 / 数字）是否匹配文本中连续若干字的首字母；`prefix` 要求从第一个字开始 */
export function matchInitials(text: string, query: string, prefix = false): boolean {
  if (!query || !/^[a-z0-9]+$/i.test(query))
    return false
  const q = query.toLowerCase()
  const seq = initialsOf(text)
  const last = prefix ? 0 : seq.length - q.length
  for (let start = 0; start <= last; start++) {
    let k = 0
    while (k < q.length && seq[start + k]?.includes(q[k]!))
      k++
    if (k === q.length)
      return true
  }
  return false
}
