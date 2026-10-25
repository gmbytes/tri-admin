import { expect, it } from 'vitest'
import {
  loadPendingCleanups,
  savePendingCleanup,
  completePendingCleanup,
  parseCleanupResult,
  type JournalStorage,
} from '../src/core/log-cleanup'
function storage(): JournalStorage {
  const data = new Map<string, string>()
  return {
    get length() {
      return data.size
    },
    key: (index) => [...data.keys()][index] ?? null,
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, value) => {
      data.set(key, value)
    },
    removeItem: (key) => {
      data.delete(key)
    },
  }
}
const scope = { endpoint: 'http://127.0.0.1:18088/api/v1', actorId: '0123456789abcdef01234567' }
const request = {
  requestId: '00000000-0000-4000-8000-000000000001',
  kind: 'operation' as const,
  before: '2026-01-01T00:00:00.000Z',
  limit: 1000,
}
it('retains exact pending inputs across page lifetimes and separates endpoints and actors', () => {
  const data = storage()
  savePendingCleanup(data, scope, request)
  expect(loadPendingCleanups(data, scope)).toEqual([request])
  expect(loadPendingCleanups(data, { ...scope, actorId: 'other-user' })).toEqual([])
  expect(loadPendingCleanups(data, { ...scope, endpoint: 'https://example.test/api/v1' })).toEqual(
    [],
  )
  const other = { ...request, requestId: '00000000-0000-4000-8000-000000000002' }
  savePendingCleanup(data, scope, other)
  completePendingCleanup(data, scope, request.requestId)
  expect(loadPendingCleanups(data, scope)).toEqual([other])
})
it('does not overwrite an uncertain action or silently ignore unavailable or corrupt storage', () => {
  const data = storage()
  savePendingCleanup(data, scope, request)
  expect(() => savePendingCleanup(data, scope, { ...request, limit: 1 })).toThrow()
  const broken = {
    ...data,
    setItem: () => {
      throw new Error('blocked')
    },
  }
  expect(() =>
    savePendingCleanup(broken, scope, {
      ...request,
      requestId: '00000000-0000-4000-8000-000000000003',
    }),
  ).toThrow()
  data.setItem(data.key(0)!, 'broken')
  expect(() => loadPendingCleanups(data, scope)).toThrow()
})
it('only accepts a result for the exact saved cleanup before clearing its marker', () => {
  const result = {
    ...request,
    deletedCount: 2,
    hasMore: true,
    completedAt: '2026-01-02T00:00:00.000Z',
  }
  expect(parseCleanupResult(result, request)).toEqual(result)
  expect(() => parseCleanupResult({ ...result, requestId: 'different' }, request)).toThrow()
  expect(() => parseCleanupResult({ ...result, deletedCount: 1001 }, request)).toThrow()
  expect(() => parseCleanupResult(null, request)).toThrow()
})
