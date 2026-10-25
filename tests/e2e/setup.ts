import { spawn, spawnSync } from 'node:child_process'
import { randomBytes } from 'node:crypto'
import { mkdir, mkdtemp, readFile, writeFile, rm } from 'node:fs/promises'
import { openSync, closeSync } from 'node:fs'
import { resolve, join, sep } from 'node:path'

export default async function setup() {
  const root = resolve('.superpowers/e2e')
  await mkdir(root, { recursive: true })
  const directory = await mkdtemp(join(root, 'run-')),
    passwordFile = join(directory, 'password'),
    metadata = join(directory, 'server.json')
  await writeFile(passwordFile, randomBytes(24).toString('hex'), { mode: 0o600 })
  const log = openSync(join(directory, 'server.log'), 'w')
  const environment = {
    ...process.env,
    CARGO_INCREMENTAL: '0',
    TRI_E2E_DATABASE: `tri_admin_e2e_${randomBytes(12).toString('hex')}`,
    TRI_E2E_PASSWORD_FILE: passwordFile,
    TRI_E2E_META_FILE: metadata,
    TRI_JWT_SIGNING_KEY: randomBytes(32).toString('hex'),
  }
  const build = spawnSync('cargo', ['build', '-p', 'admin-api', '--example', 'e2e_server'], {
    env: environment,
    windowsHide: true,
    stdio: ['ignore', log, log],
  })
  if (build.status !== 0) {
    closeSync(log)
    throw new Error('UI 测试服务编译失败，日志：' + directory)
  }
  const executable = resolve(
    'target/debug/examples/e2e_server' + (process.platform === 'win32' ? '.exe' : ''),
  )
  const child = spawn(executable, [], {
    env: environment,
    windowsHide: true,
    stdio: ['pipe', log, log],
  })
  let exited = false
  const terminal = new Promise<void>((resolve) => {
    child.once('exit', () => {
      exited = true
      resolve()
    })
  })
  const stop = async () => {
    if (!exited) {
      child.stdin.end('shutdown\n')
      await Promise.race([terminal, new Promise<void>((resolve) => setTimeout(resolve, 10_000))])
      if (!exited) {
        child.kill()
        await terminal
      }
    }
    closeSync(log)
  }
  try {
    let api = '',
      ready = false
    const deadline = Date.now() + 30_000
    while (Date.now() < deadline) {
      if (exited) throw new Error('UI 测试服务提前退出，日志：' + directory)
      try {
        api = (JSON.parse(await readFile(metadata, 'utf8')) as { api: string }).api
        if (
          (await fetch(new URL('/health/ready', api), { signal: AbortSignal.timeout(1000) })).ok
        ) {
          ready = true
          break
        }
      } catch {
        /* Wait for the tracked process to become ready. */
      }
      await new Promise((resolve) => setTimeout(resolve, 100))
    }
    if (!ready) throw new Error('UI 测试服务未就绪，日志：' + directory)
    const login = await fetch(api + '/auth/login', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ username: 'admin', password: await readFile(passwordFile, 'utf8') }),
    })
    if (!login.ok) throw new Error('UI 测试管理员初始化登录失败')
    const auth = (await login.json()).data as { accessToken: string; user: { roleIds: string[] } }
    if (!auth.accessToken || !auth.user.roleIds[0]) throw new Error('UI 测试管理员身份无效')
    process.env.TRI_E2E_PROVISIONING_TOKEN = auth.accessToken
    process.env.TRI_E2E_OWNER_ROLE = auth.user.roleIds[0]
    process.env.TRI_E2E_API = api
    process.env.TRI_E2E_PASSWORD_FILE = passwordFile
    return async () => {
      await stop()
      delete process.env.TRI_E2E_PROVISIONING_TOKEN
      delete process.env.TRI_E2E_OWNER_ROLE
      const target = resolve(directory)
      if (!target.startsWith(root + sep)) throw new Error('Refusing out-of-scope test cleanup')
      await rm(target, { recursive: true, force: true })
    }
  } catch (error) {
    await stop()
    throw error
  }
}
