/**
 * 评论输入框里的 @ 提及（纯函数）：光标前正在输入的 @关键字、选中候选后插入「@名字 」、提交时只保留文字里还在的 @
 */

export interface Mentionable {
  id: string
  name: string
}

const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

/** 光标前正在输入的 @：返回 @ 的位置与已输入的关键字；不在输入 @ 时返回 null。@ 前面必须是开头或空白 */
export function activeMention(text: string, caret: number): { start: number, keyword: string } | null {
  const before = text.slice(0, caret)
  const at = before.lastIndexOf('@')
  if (at < 0)
    return null
  if (at > 0 && !/\s/.test(before[at - 1]!))
    return null
  const keyword = before.slice(at + 1)
  // 关键字里出现空白说明这个 @ 已经结束了
  return /\s/.test(keyword) ? null : { start: at, keyword }
}

/** 选中候选：把光标前的「@关键字」替换成「@名字 」，返回新文本与新的光标位置 */
export function insertMention(text: string, caret: number, start: number, name: string): { text: string, caret: number } {
  const insert = `@${name} `
  return { text: text.slice(0, start) + insert + text.slice(caret), caret: start + insert.length }
}

/** 文字里还保留着「@名字」的被提及的人（删掉了文字就不再通知），去重 */
export function mentionsIn(text: string, picked: Mentionable[]): Mentionable[] {
  const seen = new Set<string>()
  return picked.filter((m) => {
    if (seen.has(m.id) || !new RegExp(`@${escapeRe(m.name)}(?![\\p{L}\\p{N}_])`, 'u').test(text))
      return false
    seen.add(m.id)
    return true
  })
}

/** 把评论正文里的 @名字 切成片段，界面上高亮 */
export function splitMentions(body: string, mentions: Mentionable[]): { text: string, mention: boolean }[] {
  const names = [...new Set(mentions.map(m => m.name))].sort((a, b) => b.length - a.length)
  if (!names.length)
    return [{ text: body, mention: false }]
  // 带捕获组的 split：奇数位是匹配到的 @名字
  const parts = body.split(new RegExp(`(@(?:${names.map(escapeRe).join('|')}))`, 'g'))
  return parts.map((text, i) => ({ text, mention: i % 2 === 1 })).filter(p => p.text)
}
