/** 转义后再拼进 HTML 提示（ElMessageBox 的 dangerouslyUseHTMLString） */
export function escapeHtml(s: string) {
  return s.replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]!)
}
