// 让 Node 直接运行 src 里的 TypeScript 模块：注册路径解析钩子（@/ 别名、省略的 .ts 扩展名）
import { register } from 'node:module'

register('./loader.mjs', import.meta.url)
