import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import Components from 'unplugin-vue-components/vite'
import { ElementPlusResolver } from 'unplugin-vue-components/resolvers'
import { existsSync, readdirSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join } from 'node:path'

const require = createRequire(import.meta.url)
const elementRoot = dirname(require.resolve('element-plus/package.json'))
const componentRoot = join(elementRoot, 'es/components')
const componentStyles = readdirSync(componentRoot)
  .filter((name) => existsSync(join(componentRoot, name, 'style/css.mjs')))
  .map((name) => `element-plus/es/components/${name}/style/css`)

export default defineConfig({
  plugins: [vue(), Components({ resolvers: [ElementPlusResolver()], dts: 'src/components.d.ts' })],
  clearScreen: false,
  optimizeDeps: { include: ['element-plus/es', ...componentStyles] },
  server: { port: 1420, strictPort: true },
  build: { target: 'es2022' },
})
