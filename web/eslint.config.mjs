import antfu from '@antfu/eslint-config'
import { FlatCompat } from '@eslint/eslintrc'

const compat = new FlatCompat()

export default antfu(
  {
    rules: {
      'no-console': 'off',
    },
  },
  ...compat.config({
    extends: [
      './.eslintrc-auto-import.json',
    ],
  }),
  {
    // 单元测试用 Node 内置的测试运行器（node --test），不引入 vitest 依赖
    files: ['tests/**'],
    rules: {
      'test/no-import-node-test': 'off',
    },
  },
)
