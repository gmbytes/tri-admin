import { createServer, type IncomingMessage, type ServerResponse } from 'node:http'
import { createHash } from 'node:crypto'
import { afterEach, describe, expect, it } from 'vitest'
import { ApiClient, ApiError, type SessionVault } from '../src/core/client'

const close: Array<() => Promise<void>> = []
afterEach(async () => {
  await Promise.all(close.splice(0).map((fn) => fn()))
})

async function server(handler: (req: IncomingMessage, res: ServerResponse) => void) {
  const http = createServer(handler)
  await new Promise<void>((resolve) => http.listen(0, '127.0.0.1', resolve))
  close.push(
    () =>
      new Promise<void>((resolve, reject) => {
        http.closeAllConnections()
        http.close((error) => (error ? reject(error) : resolve()))
      }),
  )
  const address = http.address()
  if (!address || typeof address === 'string') throw new Error('No test server address')
  return `http://127.0.0.1:${address.port}`
}

function send(res: ServerResponse, status: number, value: unknown) {
  res.writeHead(status, { 'content-type': 'application/json' })
  res.end(JSON.stringify(value))
}
function vault(): SessionVault {
  const values = new Map<string, string>()
  return {
    load: async (endpoint) => values.get(endpoint) ?? null,
    save: async (endpoint, token) => {
      values.set(endpoint, token)
    },
    clear: async (endpoint) => {
      values.delete(endpoint)
    },
  }
}
const user = {
  id: 'test-user',
  username: 'admin',
  displayName: 'Admin',
  roleIds: ['owner'],
  permissions: ['*'],
  dataScope: 'all',
  dataScopeRoleIds: [],
}
function tokens(accessToken = 'old-access', refreshToken = 'a'.repeat(64)) {
  return { data: { accessToken, refreshToken, expiresIn: 900, user } }
}

describe('new API session client', () => {
  it('downloads a verified configuration package through the same single refresh as JSON requests', async () => {
    let refreshes = 0
    const bytes = Buffer.from('{"format":"tri-admin/config-bundle","schemaVersion":1}')
    const checksum = createHash('sha256').update(bytes).digest('hex')
    const endpoint = await server((req, res) => {
      if (req.url?.endsWith('/login')) return send(res, 200, tokens())
      if (req.url?.endsWith('/refresh')) {
        refreshes++
        return setTimeout(() => send(res, 200, tokens('new-access')), 10)
      }
      if (req.headers.authorization === 'Bearer old-access')
        return send(res, 401, { error: { code: 'UNAUTHENTICATED', message: 'expired' } })
      if (req.url?.endsWith('/download')) {
        res.writeHead(200, {
          'content-type': 'application/json',
          'content-disposition': 'attachment; filename="tri-admin-config-proof.json"',
          etag: `"${checksum}"`,
        })
        res.end(bytes)
        return
      }
      send(res, 200, { data: 'ok' })
    })
    const client = new ApiClient({ endpoint, vault: vault() })
    await client.login('admin', 'password', false)
    const [file] = await Promise.all([
      client.downloadConfiguration('/versions/id/download'),
      client.request('/users'),
    ])
    expect(Buffer.from(file.bytes)).toEqual(bytes)
    expect(file.filename).toBe('tri-admin-config-proof.json')
    expect(file.checksum).toBe(checksum)
    expect(refreshes).toBe(1)
  })

  it('rejects damaged or oversized configuration downloads and keeps structured permission errors', async () => {
    const endpoint = await server((req, res) => {
      if (req.url?.endsWith('/login')) return send(res, 200, tokens())
      if (req.url?.endsWith('/denied')) {
        res.setHeader('x-request-id', 'download-denied')
        return send(res, 403, { error: { code: 'FORBIDDEN', message: '缺少配置读取权限' } })
      }
      res.writeHead(200, {
        'content-type': 'application/json',
        'content-disposition': 'attachment; filename="tri-admin-config-proof.json"',
        etag: `"${'0'.repeat(64)}"`,
        ...(req.url?.endsWith('/large') ? { 'content-length': String(8 * 1024 * 1024 + 1) } : {}),
      })
      res.end('{}')
    })
    const client = new ApiClient({ endpoint, vault: vault() })
    await client.login('admin', 'password', false)
    await expect(client.downloadConfiguration('/damaged')).rejects.toMatchObject({
      code: 'INVALID_DOWNLOAD',
    })
    await expect(client.downloadConfiguration('/large')).rejects.toMatchObject({
      code: 'INVALID_DOWNLOAD',
    })
    await expect(client.downloadConfiguration('/denied')).rejects.toMatchObject({
      status: 403,
      code: 'FORBIDDEN',
      requestId: 'download-denied',
    })
  })

  it('preserves selected-role scope from the new authentication contract', async () => {
    const scoped = { ...user, dataScope: 'roles', dataScopeRoleIds: ['0123456789abcdef01234567'] }
    const endpoint = await server((_req, res) =>
      send(res, 200, { data: { ...tokens().data, user: scoped } }),
    )
    const client = new ApiClient({ endpoint, vault: vault() })
    await client.login('operator', 'password', false)
    expect(client.user).toMatchObject({
      dataScope: 'roles',
      dataScopeRoleIds: scoped.dataScopeRoleIds,
    })
  })
  it('shares one refresh for concurrent expired requests including late 401 responses', async () => {
    let refreshes = 0
    let revoked = false
    const endpoint = await server((req, res) => {
      if (req.url === '/api/v1/auth/login') return send(res, 200, tokens())
      if (req.url === '/api/v1/auth/refresh') {
        refreshes++
        if (refreshes > 1) revoked = true
        return setTimeout(() => send(res, 200, tokens('new-access', 'b'.repeat(64))), 10)
      }
      if (req.headers.authorization === 'Bearer old-access') {
        return setTimeout(
          () => send(res, 401, { error: { code: 'UNAUTHENTICATED', message: 'expired' } }),
          req.url?.endsWith('late') ? 80 : 0,
        )
      }
      if (revoked) return send(res, 401, { error: { code: 'UNAUTHENTICATED', message: 'revoked' } })
      if (req.headers.authorization !== 'Bearer new-access')
        return send(res, 401, { error: { code: 'UNAUTHENTICATED', message: 'invalid' } })
      send(res, 200, { data: { name: req.url } })
    })
    const client = new ApiClient({ endpoint, vault: vault() })
    await client.login('admin', 'password', false)
    const results = await Promise.all([
      client.request<{ name: string }>('/users'),
      client.request<{ name: string }>('/late'),
    ])
    expect(results.map((result) => result.data.name)).toEqual(['/api/v1/users', '/api/v1/late'])
    expect(refreshes).toBe(1)
  })

  it('does not refresh invalid credentials and preserves structured errors', async () => {
    let refreshes = 0
    const endpoint = await server((req, res) => {
      if (req.url?.endsWith('/refresh')) refreshes++
      send(res, 401, { error: { code: 'INVALID_CREDENTIALS', message: '账号或密码错误' } })
    })
    const client = new ApiClient({ endpoint, vault: vault() })
    await expect(client.login('admin', 'wrong', false)).rejects.toMatchObject({
      status: 401,
      code: 'INVALID_CREDENTIALS',
      message: '账号或密码错误',
    })
    expect(refreshes).toBe(0)
  })

  it('clears persistent state on logout even if a refresh was already saving', async () => {
    const base = vault()
    let saving!: () => void
    let release!: () => void
    const started = new Promise<void>((resolve) => {
      saving = resolve
    })
    const released = new Promise<void>((resolve) => {
      release = resolve
    })
    const storage: SessionVault = {
      ...base,
      save: async (endpoint, token) => {
        if (token === 'b'.repeat(64)) {
          saving()
          await released
        }
        await base.save(endpoint, token)
      },
    }
    let revoked = false
    const endpoint = await server((req, res) => {
      if (req.url?.endsWith('/login')) return send(res, 200, tokens())
      if (req.url?.endsWith('/refresh')) return send(res, 200, tokens('new-access', 'b'.repeat(64)))
      if (req.url?.endsWith('/logout')) {
        let body = ''
        req.on('data', (value) => {
          body += value
        })
        req.on('end', () => {
          const value = JSON.parse(body)
          revoked = typeof value.refreshToken === 'string'
          res.writeHead(revoked ? 204 : 422)
          res.end()
        })
        return
      }
      if (req.headers.authorization === 'Bearer old-access')
        return send(res, 401, { error: { code: 'UNAUTHENTICATED', message: 'expired' } })
      send(res, 200, { data: 'ok' })
    })
    const client = new ApiClient({ endpoint, vault: storage })
    await client.login('admin', 'password', true)
    const request = client.request('/users').catch((error) => error)
    await started
    const logout = client.logout()
    release()
    expect(await logout).toBe(true)
    await request
    expect(revoked).toBe(true)
    expect(client.user).toBeNull()
    expect(await storage.load(client.endpoint)).toBeNull()
    await expect(client.request('/users')).rejects.toMatchObject({ code: 'AUTH_REQUIRED' })
  })

  it('restores a remembered session by rotating and saving the refresh credential', async () => {
    const endpoint = await server((_req, res) =>
      send(res, 200, tokens('restored-access', 'b'.repeat(64))),
    )
    const storage = vault()
    const client = new ApiClient({ endpoint, vault: storage })
    await storage.save(client.endpoint, 'a'.repeat(64))
    await client.restore()
    expect(client.user?.id).toBe('test-user')
    expect(await storage.load(client.endpoint)).toBe('b'.repeat(64))
  })

  it('discards an old server refresh when the server is switched', async () => {
    let release: (() => void) | undefined
    let refreshStarted!: () => void
    const started = new Promise<void>((resolve) => {
      refreshStarted = resolve
    })
    const first = await server((req, res) => {
      if (req.url?.endsWith('/login')) return send(res, 200, tokens())
      if (req.url?.endsWith('/refresh')) {
        refreshStarted()
        release = () => send(res, 200, tokens('late-old-server'))
        return
      }
      send(res, 401, { error: { code: 'UNAUTHENTICATED', message: 'expired' } })
    })
    const second = await server((_req, res) => send(res, 200, { data: 'second' }))
    const storage = vault()
    const client = new ApiClient({ endpoint: first, vault: storage })
    await client.login('admin', 'password', true)
    const pending = client.request('/users').catch((error) => error)
    await started
    await client.setEndpoint(second)
    release?.()
    expect(await pending).toMatchObject({ code: 'SESSION_CHANGED' })
    expect(client.user).toBeNull()
    expect(await storage.load(client.endpoint)).toBeNull()
  })

  it('rejects malformed responses without exposing raw response text', async () => {
    const endpoint = await server((_req, res) => {
      res.writeHead(200)
      res.end('private upstream diagnostic')
    })
    const client = new ApiClient({ endpoint, vault: vault() })
    const result = await client.login('admin', 'password', false).catch((error) => error)
    expect(result).toBeInstanceOf(ApiError)
    expect(result.code).toBe('INVALID_RESPONSE')
    expect(result.message).not.toContain('private upstream diagnostic')
  })

  it('rolls back a partially saved login credential when local persistence fails', async () => {
    const base = vault()
    let revoked = false
    const storage: SessionVault = {
      ...base,
      save: async (endpoint, token) => {
        await base.save(endpoint, token)
        throw new Error('private storage diagnostic')
      },
    }
    const endpoint = await server((req, res) => {
      if (req.url?.endsWith('/logout')) {
        revoked = true
        res.writeHead(204)
        res.end()
      } else send(res, 200, tokens())
    })
    const client = new ApiClient({ endpoint, vault: storage })
    await expect(client.login('admin', 'password', true)).rejects.toMatchObject({
      code: 'SESSION_STORAGE_ERROR',
    })
    expect(client.user).toBeNull()
    expect(await base.load(client.endpoint)).toBeNull()
    expect(revoked).toBe(true)
  })

  it('ends the session if a rotated credential cannot be saved safely', async () => {
    const base = vault()
    let revoked = false
    const storage: SessionVault = {
      ...base,
      save: async (endpoint, token) => {
        await base.save(endpoint, token)
        if (token === 'b'.repeat(64)) throw new Error('private storage diagnostic')
      },
    }
    const endpoint = await server((req, res) => {
      if (req.url?.endsWith('/login')) return send(res, 200, tokens())
      if (req.url?.endsWith('/refresh')) return send(res, 200, tokens('new-access', 'b'.repeat(64)))
      if (req.url?.endsWith('/logout')) {
        revoked = true
        res.writeHead(204)
        res.end()
        return
      }
      send(res, 401, { error: { code: 'UNAUTHENTICATED', message: 'expired' } })
    })
    const client = new ApiClient({ endpoint, vault: storage })
    await client.login('admin', 'password', true)
    await expect(client.request('/users')).rejects.toMatchObject({ code: 'SESSION_STORAGE_ERROR' })
    expect(client.user).toBeNull()
    expect(await base.load(client.endpoint)).toBeNull()
    expect(revoked).toBe(true)
  })
})
