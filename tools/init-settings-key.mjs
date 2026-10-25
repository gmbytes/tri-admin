import { randomBytes } from 'node:crypto'
import { open, readFile, appendFile, unlink } from 'node:fs/promises'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
export async function initializeSettingsKey(path = resolve('.env')) {
  const lockPath = path + '.settings-key.lock'
  let lock
  try {
    lock = await open(lockPath, 'wx', 0o600)
  } catch (error) {
    if (error.code === 'EEXIST') throw new Error('已有配置密钥初始化任务，请稍后重试')
    throw error
  }
  try {
    const original = await readFile(path, 'utf8'),
      hasRing = /^\s*TRI_SETTINGS_KEYS\s*=/m.test(original),
      hasActive = /^\s*TRI_SETTINGS_ACTIVE_KEY\s*=/m.test(original)
    if (hasRing && hasActive) return { created: false, path }
    if (hasRing || hasActive) throw new Error('配置密钥环境变量不完整，请检查现有设置')
    const prefix = original.endsWith('\n') ? '' : '\n'
    await appendFile(
      path,
      `${prefix}TRI_SETTINGS_KEYS=local:${randomBytes(32).toString('hex')}\nTRI_SETTINGS_ACTIVE_KEY=local\n`,
    )
    return { created: true, path }
  } finally {
    await lock.close()
    await unlink(lockPath)
  }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  initializeSettingsKey()
    .then((result) =>
      console.log(
        result.created
          ? 'Added configuration encryption key to existing .env (not printed).'
          : 'Kept existing configuration keys unchanged.',
      ),
    )
    .catch(() => {
      console.error('Configuration key initialization failed; check the environment file and lock.')
      process.exitCode = 1
    })
