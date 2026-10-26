import { randomBytes } from 'node:crypto'
import { open, readFile, appendFile, unlink } from 'node:fs/promises'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
export async function initializeOpenKey(path = resolve('.env')) {
  const lockPath = path + '.open-key.lock'
  let lock
  try {
    lock = await open(lockPath, 'wx', 0o600)
  } catch {
    throw Error('已有开放密钥初始化任务或环境目录不可用')
  }
  try {
    const original = await readFile(path, 'utf8'),
      existing = original.match(/^\s*TRI_OPEN_JWT_SIGNING_KEY\s*=([^\r\n]*)/m)
    if (existing) {
      if (existing[1].trim().replace(/^(['"])(.*)\1$/, '$2').length < 32)
        throw Error('已有开放签名密钥不完整，请检查环境配置')
      return { created: false, path }
    }
    await appendFile(
      path,
      `${original.endsWith('\n') ? '' : '\n'}TRI_OPEN_JWT_SIGNING_KEY=${randomBytes(32).toString('hex')}\n`,
    )
    return { created: true, path }
  } finally {
    await lock.close()
    await unlink(lockPath)
  }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  initializeOpenKey()
    .then((result) =>
      console.log(
        result.created
          ? 'Added independent open signing key (not printed).'
          : 'Kept existing open signing key unchanged.',
      ),
    )
    .catch(() => {
      console.error('Open signing key initialization failed; check the environment file and lock.')
      process.exitCode = 1
    })
