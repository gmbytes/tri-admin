import { randomBytes } from 'node:crypto'
import { writeFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

export async function initializeDevelopmentEnv(path = resolve('.env')) {
  const values = {
    TRI_MONGODB_URI:
      'mongodb://127.0.0.1:13317/?replicaSet=tri-admin-rs&directConnection=true&serverSelectionTimeoutMS=3000',
    TRI_MONGODB_DATABASE: 'tri_admin_dev',
    TRI_JWT_SIGNING_KEY: randomBytes(32).toString('hex'),
    TRI_ADMIN_BIND: '127.0.0.1:18088',
    TRI_SETTINGS_KEYS: `local:${randomBytes(32).toString('hex')}`,
    TRI_SETTINGS_ACTIVE_KEY: 'local',
  }
  const content =
    Object.entries(values)
      .map(([key, value]) => `${key}=${value}`)
      .join('\n') + '\n'
  try {
    await writeFile(path, content, { flag: 'wx', mode: 0o600 })
  } catch (error) {
    if (error.code === 'EEXIST') return { created: false, path }
    throw error
  }
  return { created: true, path }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  initializeDevelopmentEnv()
    .then((result) =>
      console.log(
        result.created
          ? 'Generated development .env (credentials are not printed).'
          : 'Kept existing .env unchanged.',
      ),
    )
    .catch((error) => {
      console.error(error.message)
      process.exitCode = 1
    })
}
