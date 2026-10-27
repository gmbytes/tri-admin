import { spawn, spawnSync } from 'node:child_process'
import { randomBytes } from 'node:crypto'
import { mkdir, mkdtemp, readFile, writeFile, rm } from 'node:fs/promises'
import { openSync, closeSync } from 'node:fs'
import { resolve, join, sep } from 'node:path'
import { generatedFrontend } from './generated-fixture'

export default async function setup() {
  const root = resolve('.superpowers/e2e')
  await mkdir(root, { recursive: true })
  const directory = await mkdtemp(join(root, 'run-')),
    passwordFile = join(directory, 'password'),
    programsFile = join(directory, 'programs.json'),
    metadata = join(directory, 'server.json')
  await writeFile(passwordFile, randomBytes(24).toString('hex'), { mode: 0o600 })
  const log = openSync(join(directory, 'server.log'), 'w')
  const generated = process.env.TRI_E2E_GENERATED_PROJECT
    ? await generatedFrontend(process.env.TRI_E2E_GENERATED_PROJECT)
    : null
  const environment = {
    ...process.env,
    CARGO_INCREMENTAL: '0',
    TRI_E2E_DATABASE: `tri_admin_e2e_${randomBytes(12).toString('hex')}`,
    TRI_E2E_PASSWORD_FILE: passwordFile,
    TRI_E2E_META_FILE: metadata,
    TRI_JWT_SIGNING_KEY: randomBytes(32).toString('hex'),
    TRI_OPEN_JWT_SIGNING_KEY: randomBytes(32).toString('hex'),
    TRI_SETTINGS_KEYS: `fixture:${randomBytes(32).toString('hex')}`,
    TRI_SETTINGS_ACTIVE_KEY: 'fixture',
    TRI_STORAGE_ROOT: join(directory, 'storage'),
    TRI_MCP_PROGRAMS_FILE: programsFile,
    TRI_MCP_ALLOWED_HOSTS: '127.0.0.1,localhost',
    ...(generated
      ? {
          TRI_E2E_RESOURCE_SCHEMA_FILE: generated.schemaPath,
          TRI_E2E_RESOURCE_ORIGIN: generated.origin,
          TRI_E2E_RESOURCE_KEY: randomBytes(32).toString('hex'),
        }
      : {}),
  }
  const build = spawnSync(
    'cargo',
    ['build', '-p', 'admin-api', '--example', 'e2e_server', '--bin', 'tri-admin-mcp-test-peer'],
    {
      env: environment,
      windowsHide: true,
      stdio: ['ignore', log, log],
    },
  )
  if (build.status !== 0) {
    await generated?.close()
    closeSync(log)
    throw new Error('UI 测试服务编译失败，日志：' + directory)
  }
  const peerDirectory = join(directory, 'mcp-records')
  await mkdir(peerDirectory)
  await writeFile(
    programsFile,
    JSON.stringify({
      formatVersion: 1,
      programs: [
        {
          id: 'e2e-records',
          label: '受控 E2E 记录服务',
          program: resolve(
            'target/debug/tri-admin-mcp-test-peer' + (process.platform === 'win32' ? '.exe' : ''),
          ),
          arguments: ['normal', peerDirectory],
          workingDirectory: peerDirectory,
          environment: process.platform === 'win32' ? { SystemRoot: process.env.SystemRoot } : {},
          allowedTools: ['increment', 'inspect', 'effect_then_wait'],
          startupTimeoutMs: 5000,
          requestTimeoutMs: 5000,
          stopTimeoutMs: 3000,
        },
      ],
    }),
    { mode: 0o600 },
  )
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
    await generated?.close()
  }
  try {
    let api = '',
      resourceApi = '',
      ready = false
    const deadline = Date.now() + 30_000
    while (Date.now() < deadline) {
      if (exited) throw new Error('UI 测试服务提前退出，日志：' + directory)
      try {
        const info = JSON.parse(await readFile(metadata, 'utf8')) as {
          api: string
          resourceApi?: string
        }
        api = info.api
        resourceApi = info.resourceApi ?? ''
        if (
          (await fetch(new URL('/health/ready', api), { signal: AbortSignal.timeout(1000) })).ok &&
          (!generated ||
            (resourceApi &&
              (
                await fetch(new URL('/health/ready', resourceApi), {
                  signal: AbortSignal.timeout(1000),
                })
              ).ok))
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
    if (generated) {
      process.env.TRI_E2E_GENERATED_ORIGIN = generated.origin
      process.env.TRI_E2E_GENERATED_API = resourceApi
    }
    return async () => {
      await stop()
      delete process.env.TRI_E2E_PROVISIONING_TOKEN
      delete process.env.TRI_E2E_OWNER_ROLE
      delete process.env.TRI_E2E_GENERATED_ORIGIN
      delete process.env.TRI_E2E_GENERATED_API
      const target = resolve(directory)
      if (!target.startsWith(root + sep)) throw new Error('Refusing out-of-scope test cleanup')
      await rm(target, { recursive: true, force: true })
    }
  } catch (error) {
    await stop()
    throw error
  }
}
