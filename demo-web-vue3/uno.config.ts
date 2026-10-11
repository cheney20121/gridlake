import type { UserConfig } from 'unocss'
import process from 'node:process'
import presetIcons from '@unocss/preset-icons'
import transformerDirectives from '@unocss/transformer-directives'
import { defineConfig, presetUno } from 'unocss'

const namespace = process.env.VITE_ELEMENT_NAMESPACE || 'ec'

export default defineConfig({
  transformers: [
    transformerDirectives(),
  ],
  theme: {
    colors: {
      primary: {
        700: `var(--${namespace}-color-primary-dark-2)`,
        600: `var(--${namespace}-color-primary)`,
        500: `var(--${namespace}-color-primary-light-3)`,
        400: `var(--${namespace}-color-primary-light-5)`,
        300: `var(--${namespace}-color-primary-light-7)`,
        200: `var(--${namespace}-color-primary-light-8)`,
        100: `var(--${namespace}-color-primary-light-9)`,
        50: `var(--${namespace}-color-primary-light-9)`,
      },
      title: {
        main: '#101828',
        sub: '#667085',
      },
      text: {
        primary: 'var(--color-text-primary)',
        regular: 'var(--color-text-regular)',
      },
      warning: 'var(--color-warning)',
      // regular: '#344054',
    },
  },
  blocklist: [
    'container',
  ],
  shortcuts: [
    ['sample', ['bg-red']],
    ['icon-wrap', 'flex justify-center hover:bg-gray-100 rounded h-8 w-8 items-center cursor-pointer'],
  ],
  presets: [
    presetUno(),
    presetIcons({
      extraProperties: {
        'display': 'inline-block',
        'vertical-align': 'base-inline',
      },
      autoInstall: true,
    }),
  ],
} as UserConfig)
