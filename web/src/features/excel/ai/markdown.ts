/**
 * AI 回答的简易 Markdown → HTML：先转义全部 HTML，再处理有限的语法（安全，不执行模型输出的任何标签）
 * 支持：标题（#）、粗体（**）、行内代码（`）、无序 / 有序列表、Markdown 表格（| a | b |）、制表符分隔的表格、段落换行
 */

function esc(s: string) {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')
}

function inline(s: string) {
  return esc(s)
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
}

const isNum = (s: string) => /^[-+]?[\d,]+(?:\.\d+)?%?$|^[-+]?[\d.]+万$/.test(s.trim())

function table(rows: string[][]) {
  const [head, ...body] = rows
  const cell = (c: string, tag: 'th' | 'td') => `<${tag}${tag === 'td' && isNum(c) ? ' class="num"' : ''}>${inline(c.trim())}</${tag}>`
  return `<table><thead><tr>${head!.map(c => cell(c, 'th')).join('')}</tr></thead><tbody>${body.map(r => `<tr>${r.map(c => cell(c, 'td')).join('')}</tr>`).join('')}</tbody></table>`
}

export function renderMarkdown(src: string): string {
  const lines = src.replace(/\r/g, '').split('\n')
  const out: string[] = []
  let i = 0
  while (i < lines.length) {
    const line = lines[i]!
    // Markdown 表格
    if (/^\s*\|.*\|\s*$/.test(line)) {
      const rows: string[][] = []
      while (i < lines.length && /^\s*\|.*\|\s*$/.test(lines[i]!)) {
        const cells = lines[i]!.trim().slice(1, -1).split('|')
        if (!cells.every(c => /^\s*:?-{2,}:?\s*$/.test(c)))
          rows.push(cells)
        i++
      }
      out.push(table(rows))
      continue
    }
    // 制表符分隔（工具结果被原样引用时）
    if (line.includes('\t') && (lines[i + 1] ?? '').includes('\t')) {
      const rows: string[][] = []
      while (i < lines.length && lines[i]!.includes('\t')) {
        rows.push(lines[i]!.split('\t'))
        i++
      }
      out.push(table(rows))
      continue
    }
    const h = /^#{1,4}\s(.*)$/.exec(line)
    if (h) {
      out.push(`<div class="h">${inline(h[1]!.trim())}</div>`)
      i++
      continue
    }
    if (/^\s*(?:[-*•]|\d+[.、)])\s+/.test(line)) {
      const ordered = /^\s*\d/.test(line)
      const items: string[] = []
      while (i < lines.length && /^\s*(?:[-*•]|\d+[.、)])\s+/.test(lines[i]!)) {
        items.push(`<li>${inline(lines[i]!.replace(/^\s*(?:[-*•]|\d+[.、)])\s+/, ''))}</li>`)
        i++
      }
      out.push(ordered ? `<ol>${items.join('')}</ol>` : `<ul>${items.join('')}</ul>`)
      continue
    }
    if (!line.trim()) {
      i++
      continue
    }
    const para: string[] = []
    while (i < lines.length && lines[i]!.trim() && !/^\s*\|/.test(lines[i]!) && !/^#{1,4}\s/.test(lines[i]!) && !/^\s*(?:[-*•]|\d+[.、)])\s+/.test(lines[i]!) && !(lines[i]!.includes('\t') && (lines[i + 1] ?? '').includes('\t'))) {
      para.push(inline(lines[i]!))
      i++
    }
    out.push(`<p>${para.join('<br>')}</p>`)
  }
  return out.join('')
}
