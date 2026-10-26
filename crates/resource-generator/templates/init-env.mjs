import { randomBytes } from 'node:crypto'
import { readFile, writeFile } from 'node:fs/promises'
const root = new URL('../', import.meta.url),
  schema = JSON.parse(await readFile(new URL('schema.json', root), 'utf8'))
const values = {
  TRI_MONGODB_URI: '',
  TRI_MONGODB_DATABASE: schema.resource + '_dev',
  TRI_SIGNING_KEY: randomBytes(32).toString('hex'),
  TRI_BIND: '127.0.0.1:18089',
  TRI_ORIGIN: 'http://localhost:5173',
}
try {
  await writeFile(
    new URL('.env', root),
    Object.entries(values)
      .map(([key, value]) => `${key}=${value}`)
      .join('\n') + '\n',
    { flag: 'wx', mode: 0o600 },
  )
  console.log('已生成独立签名密钥；请在 .env 中填写 MongoDB 副本集地址。')
} catch (error) {
  if (error.code !== 'EEXIST') throw error
  console.log('已有 .env 保持不变。')
}
