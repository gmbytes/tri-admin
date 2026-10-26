import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { Client, normalizeEndpoint } from '../src/client.mjs'
const id = '0123456789abcdef01234567',
  schema = { resource: 'inventory', fields: [{ name: 'name', kind: 'text', required: true }] }
const user = { id, username: 'owner', canWrite: true, dataScope: 'all' },
  row = {
    id,
    ownerId: id,
    version: 1,
    createdAt: '2026-10-26T00:00:00.000Z',
    updatedAt: '2026-10-26T00:00:00.000Z',
    values: { name: 'sample' },
  }
function reply(response, status, data) {
  response.writeHead(status, { 'content-type': 'application/json' })
  response.end(data === null ? '' : JSON.stringify(data))
}
async function fixture(handler) {
  const server = createServer(handler)
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  return {
    origin: `http://127.0.0.1:${server.address().port}`,
    close: async () => {
      server.closeAllConnections()
      await new Promise((resolve) => server.close(resolve))
    },
  }
}
test('endpoint policy rejects embedded secrets and non TLS remote servers', () => {
  assert.equal(normalizeEndpoint('http://127.0.0.1:18089/'), 'http://127.0.0.1:18089/api/v1')
  for (const value of [
    'http://remote.example',
    'https://user:password@example.test',
    'https://example.test/?token=private',
    'https://example.test/other',
  ])
    assert.throws(() => normalizeEndpoint(value))
})
test('real HTTP requests keep credentials in memory and reject private or invalid resource DTOs', async () => {
  let privateReply = false,
    authorization = ''
  const server = await fixture((request, response) => {
    authorization = request.headers.authorization ?? ''
    if (request.url === '/api/v1/auth/login')
      return reply(response, 200, {
        data: { accessToken: 'fixture.access.signature', expiresIn: 900, user },
      })
    if (request.url === '/api/v1/auth/logout') return reply(response, 204, null)
    reply(response, 200, {
      data: [privateReply ? { ...row, passwordHash: 'must-not-surface' } : row],
      meta: { page: 1, pageSize: 20, total: 1 },
    })
  })
  try {
    const client = new Client(schema, server.origin)
    await client.login('owner', 'only-fixture')
    assert.equal(client.user.username, 'owner')
    assert.equal((await client.list()).data.length, 1)
    assert.equal(authorization, 'Bearer fixture.access.signature')
    privateReply = true
    await assert.rejects(
      client.list(),
      (error) => error.code === 'INVALID_RESPONSE' && !error.message.includes('must-not-surface'),
    )
    await client.logout()
    assert.equal(client.user, null)
    await assert.rejects(client.list(), (error) => error.code === 'AUTH_REQUIRED')
  } finally {
    await server.close()
  }
})
test('a late login cannot attach its identity to a changed server', async () => {
  let entered
  const started = new Promise((resolve) => {
    entered = resolve
  })
  let release
  const held = new Promise((resolve) => {
    release = resolve
  })
  const server = await fixture(async (request, response) => {
    entered()
    await held
    reply(response, 200, {
      data: { accessToken: 'fixture.access.signature', expiresIn: 900, user },
    })
  })
  try {
    const client = new Client(schema, server.origin),
      pending = client.login('owner', 'only-fixture')
    await started
    client.setEndpoint(server.origin.replace('127.0.0.1', 'localhost'))
    release()
    await assert.rejects(pending, (error) => error.code === 'SESSION_CHANGED')
    assert.equal(client.user, null)
    await assert.rejects(client.list(), (error) => error.code === 'AUTH_REQUIRED')
  } finally {
    release()
    await server.close()
  }
})
test('aggregate values are bounded before sending a write', async () => {
  let writes = 0
  const fields = Array.from({ length: 20 }, (_, index) => ({
      name: 'field' + index,
      kind: 'text',
      required: false,
    })),
    values = Object.fromEntries(fields.map((field) => [field.name, 'x'.repeat(4096)]))
  const server = await fixture((request, response) => {
    if (request.url === '/api/v1/auth/login')
      return reply(response, 200, {
        data: { accessToken: 'fixture.access.signature', expiresIn: 900, user },
      })
    writes++
    reply(response, 200, { data: row })
  })
  try {
    const client = new Client({ resource: 'bounded', fields }, server.origin)
    await client.login('owner', 'only-fixture')
    await assert.rejects(client.create(values), (error) => error.code === 'INVALID_REQUEST')
    assert.equal(writes, 0)
  } finally {
    await server.close()
  }
})
