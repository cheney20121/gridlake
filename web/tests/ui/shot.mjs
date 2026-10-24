// 手动截图：node tests/ui/shot.mjs <用户 alice|bob|root> <路径> <输出.png> [等待毫秒] [步骤...]
// 步骤：「选择器::文字」点击第一个含该文字的元素；「eval::JS」在页面里执行一段脚本
// 依赖 serve.mjs 已在运行；用于改界面时看效果
import process from 'node:process'
import { launchChrome } from './cdp.mjs'
import { TOKEN, WEB } from './infra.mjs'

async function main() {
  const [user, url, out, wait = '3000', ...clicks] = process.argv.slice(2)
  const browser = await launchChrome()
  try {
    const page = await browser.newPage()
    await page.setCookie('gridlake-token', TOKEN[user], WEB)
    await page.goto(`${WEB}${url}`)
    await new Promise(r => setTimeout(r, Number(wait)))
    for (const c of clicks) {
      if (c.startsWith('eval::')) {
        await page.eval(c.slice(6))
      }
      else {
        const [selector, text] = c.split('::')
        await page.click(selector, text)
      }
      await new Promise(r => setTimeout(r, 1200))
    }
    await page.screenshot(out)
  }
  finally {
    await browser.close()
  }
}

main()
