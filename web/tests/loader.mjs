// 路径解析钩子：只服务于单元测试，让 Node 能直接加载 src 下的 .ts 文件（不引入任何依赖）
// - `@/xxx` → src/xxx（与 vite 的别名一致）
// - 相对路径省略扩展名时补 .ts / /index.ts
import { existsSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const SRC = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../src')

export async function resolve(specifier, context, next) {
  let target = null
  if (specifier.startsWith('@/'))
    target = path.join(SRC, specifier.slice(2))
  else if ((specifier.startsWith('./') || specifier.startsWith('../')) && context.parentURL?.startsWith('file:'))
    target = path.resolve(path.dirname(fileURLToPath(context.parentURL)), specifier)
  if (target && !path.extname(target)) {
    for (const candidate of [`${target}.ts`, path.join(target, 'index.ts')]) {
      if (existsSync(candidate))
        return next(pathToFileURL(candidate).href, context)
    }
  }
  return next(specifier, context)
}
