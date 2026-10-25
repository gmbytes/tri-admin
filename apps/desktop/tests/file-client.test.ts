import { createServer, type IncomingMessage, type ServerResponse } from 'node:http'
import { createHash } from 'node:crypto'
import { afterEach, describe, expect, it } from 'vitest'
import { ApiClient, type SessionVault } from '../src/core/client'
import { createBrowserFileSink } from '../src/core/files'

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
  if (!address || typeof address === 'string') throw new Error('missing fixture address')
  return `http://127.0.0.1:${address.port}`
}
function send(res: ServerResponse, status: number, data: unknown) {
  res.writeHead(status, { 'content-type': 'application/json' })
  res.end(JSON.stringify(data))
}
function tokens(accessToken = 'old-access') {
  return {
    data: {
      accessToken,
      refreshToken: 'a'.repeat(64),
      expiresIn: 900,
      user: {
        id: 'actor',
        username: 'operator',
        displayName: 'Operator',
        roleIds: [],
        permissions: ['files.read', 'files.write'],
        dataScope: 'self',
        dataScopeRoleIds: [],
      },
    },
  }
}
const vault: SessionVault = { load: async () => null, save: async () => {}, clear: async () => {} }
const id = '0123456789abcdef01234567'
const digest = (bytes: Uint8Array) => createHash('sha256').update(bytes).digest('hex')

describe('authenticated binary file requests', () => {
  it('does not replay a file write when a refresh returns a different actor', async () => {
    let writes = 0,
      revocations = 0
    const endpoint = await server((req, res) => {
      if (req.url?.endsWith('/login')) return send(res, 200, tokens())
      if (req.url?.endsWith('/refresh')) {
        const rotated = tokens('new-access')
        rotated.data.user.id = 'another-actor'
        return send(res, 200, rotated)
      }
      if (req.url?.endsWith('/logout')) {
        revocations++
        res.writeHead(204)
        res.end()
        return
      }
      req.resume()
      if (req.headers.authorization === 'Bearer old-access')
        return send(res, 401, { error: { code: 'UNAUTHENTICATED', message: 'expired' } })
      writes++
      send(res, 200, { data: 'unexpected' })
    })
    const client = new ApiClient({ endpoint, vault })
    await client.login('operator', 'fixture', false)
    const bytes = new Uint8Array([1])
    await expect(client.putChunk(id, 0, new Blob([bytes]), digest(bytes))).rejects.toMatchObject({
      code: 'INVALID_RESPONSE',
    })
    expect(writes).toBe(0)
    expect(revocations).toBe(1)
    expect(client.user).toBeNull()
  })
  it('streams a verified download through the refresh shared with JSON requests', async () => {
    const data = Buffer.alloc(1024 * 1024 + 17, 63),
      spec = { size: data.length, sha256: digest(data) }
    let refreshes = 0
    const endpoint = await server((req, res) => {
      if (req.url?.endsWith('/login')) return send(res, 200, tokens())
      if (req.url?.endsWith('/refresh')) {
        refreshes++
        return setTimeout(() => send(res, 200, tokens('new-access')), 10)
      }
      if (req.headers.authorization === 'Bearer old-access')
        return send(res, 401, { error: { code: 'UNAUTHENTICATED', message: 'expired' } })
      if (req.url?.endsWith('/content')) {
        res.writeHead(200, {
          'content-type': 'application/octet-stream',
          'content-disposition': 'attachment; filename="proof.bin"',
          'content-length': data.length,
          etag: `"${spec.sha256}"`,
        })
        res.end(data)
      } else send(res, 200, { data: 'ok' })
    })
    const client = new ApiClient({ endpoint, vault })
    await client.login('operator', 'fixture', false)
    const [blob] = await Promise.all([
      client.downloadFile(id, spec, createBrowserFileSink(spec)),
      client.request('/users'),
    ])
    expect(Buffer.from(await blob.arrayBuffer()).equals(data)).toBe(true)
    expect(refreshes).toBe(1)
  })

  it('preserves download permission errors and refuses redirects before contacting another origin', async () => {
    let foreignRequests = 0,
      redirect = false
    const foreign = await server((_req, res) => {
      foreignRequests++
      send(res, 200, { data: 'unexpected' })
    })
    const endpoint = await server((req, res) => {
      if (req.url?.endsWith('/login')) return send(res, 200, tokens())
      if (redirect) {
        res.writeHead(302, { location: foreign + '/receive' })
        res.end()
        return
      }
      res.setHeader('x-request-id', 'download-proof')
      send(res, 403, { error: { code: 'FORBIDDEN', message: '无权下载' } })
    })
    const client = new ApiClient({ endpoint, vault })
    await client.login('operator', 'fixture', false)
    const spec = { size: 1, sha256: digest(new Uint8Array([1])) },
      aborted: number[] = []
    const sink = () => ({
      ...createBrowserFileSink(spec),
      abort: async () => {
        aborted.push(1)
      },
    })
    await expect(client.downloadFile(id, spec, sink())).rejects.toMatchObject({
      status: 403,
      code: 'FORBIDDEN',
      requestId: 'download-proof',
    })
    redirect = true
    await expect(client.downloadFile(id, spec, sink())).rejects.toMatchObject({
      code: 'NETWORK_ERROR',
    })
    expect(aborted).toHaveLength(2)
    expect(foreignRequests).toBe(0)
  })

  it('discards a partially written download when the session changes', async () => {
    let written!: () => void
    const started = new Promise<void>((resolve) => {
        written = resolve
      }),
      data = new Uint8Array([1, 2, 3])
    const spec = { size: 3, sha256: digest(data) }
    const endpoint = await server((req, res) => {
      if (req.url?.endsWith('/login')) return send(res, 200, tokens())
      res.writeHead(200, {
        'content-type': 'application/octet-stream',
        'content-disposition': 'attachment',
        'content-length': 3,
        etag: `"${spec.sha256}"`,
      })
      res.write(data.subarray(0, 1))
    })
    const client = new ApiClient({ endpoint, vault })
    await client.login('operator', 'fixture', false)
    let aborts = 0,
      completed = false
    const pending = client
      .downloadFile(id, spec, {
        write: async () => {
          written()
        },
        complete: async () => {
          completed = true
        },
        abort: async () => {
          aborts++
        },
      })
      .catch((error) => error)
    await started
    await client.clearSession()
    expect(await pending).toMatchObject({ code: 'SESSION_CHANGED' })
    expect(aborts).toBe(1)
    expect(completed).toBe(false)
  })

  it('lets a user cancel a download while retaining their session', async () => {
    let written!: () => void
    const started = new Promise<void>((resolve) => {
        written = resolve
      }),
      data = new Uint8Array([1, 2, 3]),
      spec = { size: 3, sha256: digest(data) }
    const endpoint = await server((req, res) => {
      if (req.url?.endsWith('/login')) return send(res, 200, tokens())
      res.writeHead(200, {
        'content-type': 'application/octet-stream',
        'content-disposition': 'attachment',
        'content-length': 3,
        etag: `"${spec.sha256}"`,
      })
      res.write(data.subarray(0, 1))
    })
    const client = new ApiClient({ endpoint, vault })
    await client.login('operator', 'fixture', false)
    const controller = new AbortController()
    let aborted = 0
    const pending = client
      .downloadFile(
        id,
        spec,
        {
          write: async () => {
            written()
          },
          complete: async () => {},
          abort: async () => {
            aborted++
          },
        },
        { signal: controller.signal },
      )
      .catch((error) => error)
    await started
    controller.abort()
    expect(await pending).toMatchObject({ code: 'REQUEST_CANCELLED' })
    expect(aborted).toBe(1)
    expect(client.user?.id).toBe('actor')
  })
  it('replays immutable binary chunks through the same refresh as concurrent JSON requests', async () => {
    const payload = Buffer.alloc(4 * 1024 * 1024, 71)
    const received: Buffer[] = []
    let refreshes = 0
    const endpoint = await server((req, res) => {
      if (req.url?.endsWith('/login')) return send(res, 200, tokens())
      if (req.url?.endsWith('/refresh')) {
        refreshes++
        return setTimeout(() => send(res, 200, tokens('new-access')), 10)
      }
      if (req.headers.authorization === 'Bearer old-access') {
        req.resume()
        return setTimeout(
          () => send(res, 401, { error: { code: 'UNAUTHENTICATED', message: 'expired' } }),
          req.url?.endsWith('late') ? 80 : 0,
        )
      }
      if (req.method === 'PUT') {
        expect(req.headers['content-type']).toBe('application/octet-stream')
        expect(req.headers['x-chunk-sha256']).toBe(digest(payload))
        const parts: Buffer[] = []
        req.on('data', (part) => parts.push(part))
        req.on('end', () => {
          received.push(Buffer.concat(parts))
          send(res, 200, { data: { index: 0, size: payload.length } })
        })
      } else send(res, 200, { data: 'ok' })
    })
    const client = new ApiClient({ endpoint, vault })
    await client.login('operator', 'fixture', false)
    const result = await Promise.all([
      client.putChunk(id, 0, new Blob([payload]), digest(payload)),
      client.request('/late'),
    ])
    expect(result[0].data).toMatchObject({ index: 0, size: payload.length })
    expect(received).toHaveLength(1)
    expect(received[0]!.equals(payload)).toBe(true)
    expect(refreshes).toBe(1)
  })

  it('validates chunk geometry and digest before sending bytes', async () => {
    let requests = 0
    const endpoint = await server((_req, res) => {
      requests++
      send(res, 200, tokens())
    })
    const client = new ApiClient({ endpoint, vault })
    await client.login('operator', 'fixture', false)
    const before = requests
    for (const [upload, index, blob, hash] of [
      ['../outside', 0, new Blob(['data']), 'a'.repeat(64)],
      [id, -1, new Blob(['data']), 'a'.repeat(64)],
      [id, 1280, new Blob(['data']), 'a'.repeat(64)],
      [id, 0, new Blob([]), 'a'.repeat(64)],
      [id, 0, new Blob([new Uint8Array(4 * 1024 * 1024 + 1)]), 'a'.repeat(64)],
      [id, 0, new Blob(['data']), 'invalid'],
    ] as const) {
      await expect(client.putChunk(upload, index, blob, hash)).rejects.toMatchObject({
        code: 'INVALID_FILE_CHUNK',
      })
    }
    expect(requests).toBe(before)
  })

  it('cancels an in-flight chunk when the session changes and never retries it for the next actor', async () => {
    let start!: () => void
    const started = new Promise<void>((resolve) => {
      start = resolve
    })
    let writes = 0
    const endpoint = await server((req, res) => {
      if (req.url?.endsWith('/login')) return send(res, 200, tokens())
      if (req.method === 'PUT') {
        writes++
        req.resume()
        start()
        return
      }
      send(res, 204, null)
    })
    const client = new ApiClient({ endpoint, vault })
    await client.login('operator', 'fixture', false)
    const payload = new Uint8Array([1, 2, 3])
    const pending = client
      .putChunk(id, 0, new Blob([payload]), digest(payload))
      .catch((error) => error)
    await started
    await client.clearSession()
    expect(await pending).toMatchObject({ code: 'SESSION_CHANGED' })
    expect(writes).toBe(1)
  })
})
