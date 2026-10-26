import { describe, it, expect } from 'vitest'
import {
  parseToken,
  parseApplication,
  parseIssue,
  parsePage,
  CredentialRecoveryStore,
  parseApplicationUser,
  parseRotation,
  parseCall,
} from '../src/core/open-access'
const id = '0123456789abcdef01234567',
  now = '2026-10-26T00:00:00.000Z',
  request = '00000000-0000-4000-8000-000000000001'
const token = {
  id,
  requestId: request,
  name: 'Automation',
  scopes: ['parameters.read'],
  allowedIps: [],
  minuteLimit: 600,
  dailyQuota: 100000,
  status: 'active',
  createdAt: now,
  expiresAt: '2026-11-25T00:00:00.000Z',
  revokedAt: null,
  lastUsedAt: null,
}
describe('new open access DTOs and recovery boundaries', () => {
  it('validates exact public metadata and does not coerce identifiers or leak private fields', () => {
    expect(parseToken(token)).toEqual(token)
    for (const value of [
      { ...token, id: [id] },
      { ...token, status: ['active'] },
      { ...token, secretHash: 'private' },
      { ...token, minuteLimit: 0 },
      { ...token, expiresAt: 'invalid' },
    ])
      expect(() => parseToken(value)).toThrow()
    expect(
      parsePage({ data: [token], meta: { page: 1, pageSize: 20, total: 1 } }, parseToken).data,
    ).toHaveLength(1)
    expect(() =>
      parsePage({ data: [token], meta: { page: 1, pageSize: 20, total: 0 } }, parseToken),
    ).toThrow()
  })
  it('only accepts a secret for the correct issued identity and exact result type', () => {
    const secret = `tri_at1.${id}.${'A'.repeat(43)}`
    expect(parseIssue({ token, secret }, 'token').secret).toBe(secret)
    expect(parseIssue({ token, secret: null }, 'token').secret).toBeNull()
    expect(() =>
      parseIssue({ token, secret: `tri_app1.${id}.${'A'.repeat(43)}` }, 'token'),
    ).toThrow()
    expect(() => parseIssue({ token, secret, extra: 'private' }, 'token')).toThrow()
  })
  it('keeps only immutable operation markers per normalized server and actor', () => {
    const values = new Map<string, string>(),
      storage = {
        getItem: (key: string) => values.get(key) ?? null,
        setItem: (key: string, value: string) => {
          values.set(key, value)
        },
        removeItem: (key: string) => {
          values.delete(key)
        },
      }
    const store = new CredentialRecoveryStore(storage, 'http://127.0.0.1:18088/api/v1', id)
    store.put({
      requestId: request,
      kind: 'token.create',
      applicationId: null,
      version: null,
      createdAt: now,
    })
    expect(store.read()).toHaveLength(1)
    expect(
      new CredentialRecoveryStore(storage, 'http://127.0.0.1:18089/api/v1', id).read(),
    ).toEqual([])
    expect(new CredentialRecoveryStore(storage, 'http://127.0.0.1:18088', id).read()).toHaveLength(
      1,
    )
    expect(
      new CredentialRecoveryStore(
        storage,
        'http://127.0.0.1:18088',
        '1123456789abcdef01234567',
      ).read(),
    ).toEqual([])
    expect(() =>
      store.put({
        requestId: request,
        kind: 'app.rotate',
        applicationId: id,
        version: 1,
        createdAt: now,
      }),
    ).toThrow()
    const raw = [...values.values()][0]!
    expect(raw).not.toContain('scopes')
    expect(raw).not.toContain('secret')
    expect(raw).not.toContain('password')
    values.set(
      [...values.keys()][0]!,
      JSON.stringify({
        schemaVersion: 1,
        operations: [{ ...store.read()[0], secret: 'must-not-store' }],
      }),
    )
    expect(() => store.read()).toThrow()
  })
  it('rejects app users with service scopes or password material', () => {
    const user = {
      id,
      applicationId: id,
      username: 'player.one',
      displayName: 'Player',
      scopes: ['open.profile.read'],
      status: 'active',
      version: 1,
      createdAt: now,
      updatedAt: now,
    }
    expect(parseApplicationUser(user).username).toBe('player.one')
    expect(() => parseApplicationUser({ ...user, status: ['active'] })).toThrow()
    expect(() => parseApplicationUser({ ...user, scopes: ['open.users.write'] })).toThrow()
    expect(() => parseApplicationUser({ ...user, passwordHash: 'private' })).toThrow()
    expect(() => parseApplication({ id })).toThrow()
  })
  it('binds rotation metadata to a completed version and rejects coerced log states', () => {
    const application = {
      id,
      requestId: request,
      name: 'Game',
      code: 'game.one',
      enabled: true,
      allowedScopes: ['open.profile.read'],
      allowedIps: [],
      minuteLimit: 600,
      dailyQuota: 100000,
      credentialVersion: 2,
      version: 2,
      createdAt: now,
      updatedAt: now,
      deletedAt: null,
    }
    const receipt = {
      applicationId: id,
      requestId: request,
      kind: 'rotateSecret',
      version: 2,
      credentialVersion: 2,
      createdAt: now,
    }
    expect(parseRotation({ application, receipt, secret: null }).receipt.version).toBe(2)
    expect(() =>
      parseRotation({ application: { ...application, version: 1 }, receipt, secret: null }),
    ).toThrow()
    const call = {
      id,
      tokenId: id,
      requestId: request,
      operationId: null,
      method: 'GET',
      route: '/api/v1/parameters',
      ip: null,
      state: 'started',
      status: null,
      errorCode: null,
      durationMs: null,
      createdAt: now,
      finishedAt: null,
    }
    expect(parseCall(call, 'token').state).toBe('started')
    expect(() => parseCall({ ...call, state: ['started'] }, 'token')).toThrow()
  })
})
