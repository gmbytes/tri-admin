import { it, expect } from 'vitest'
import {
  parseExportJob,
  parseExportPage,
  parseExportAction,
  ExportRecoveryStore,
  exportInput,
} from '../src/core/export-jobs'
const id = '1'.repeat(24),
  requestId = '00000000-0000-4000-8000-000000000001',
  time = new Date(0).toISOString()
const base = {
  id,
  requestId,
  resource: 'parameters',
  format: 'csv',
  maxRows: 100000,
  template: null,
  state: 'queued',
  rowCount: 0,
  file: null,
  lastError: null,
  cleanupPending: false,
  version: 1,
  createdAt: time,
  updatedAt: time,
  startedAt: null,
  completedAt: null,
  expiresAt: null,
}
it('parses the new task contract without private fields or permissive legacy shapes', () => {
  expect(parseExportJob(base).state).toBe('queued')
  const completed = {
    ...base,
    state: 'completed',
    rowCount: 1,
    file: {
      name: `tri-admin-parameters-${id}.csv`,
      mimeType: 'text/csv;charset=utf-8',
      size: 10,
      sha256: 'a'.repeat(64),
    },
    completedAt: time,
    expiresAt: new Date(86400000).toISOString(),
  }
  expect(parseExportJob(completed).file?.size).toBe(10)
  for (const change of [
    { leaseToken: 'secret' },
    { id: [id] },
    { requestId: [requestId] },
    { state: 'done' },
    { rowCount: 100001 },
    { format: 'sql' },
    { resource: 'customers' },
    { createdAt: 0 },
    { file: { ...completed.file, name: '../private' } },
    { file: { ...completed.file, size: 5 * 1024 ** 3 + 1 } },
    { file: { ...completed.file, sha256: ['a'.repeat(64)] } },
  ])
    expect(() => parseExportJob({ ...completed, ...change })).toThrow()
  expect(() => parseExportJob({ ...base, state: 'expired', file: completed.file })).toThrow()
  const page = parseExportPage(
    { data: [base], meta: { page: 1, pageSize: 20, total: 1 } },
    { page: 1, pageSize: 20 },
  )
  expect(page.total).toBe(1)
  expect(() =>
    parseExportPage(
      { data: [base], meta: { page: 2, pageSize: 20, total: 1 } },
      { page: 1, pageSize: 20 },
    ),
  ).toThrow()
  expect(
    parseExportAction({ jobId: id, requestId, kind: 'retry', generation: 1, createdAt: time }).kind,
  ).toBe('retry')
})
it('stores only bounded server and actor scoped operation identifiers and rejects corrupted records', () => {
  const values = new Map<string, string>()
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value)
    },
    removeItem: (key: string) => {
      values.delete(key)
    },
  }
  const first = new ExportRecoveryStore(storage, 'http://127.0.0.1:18088/api/v1', id)
  first.put({ requestId, kind: 'create', jobId: null, createdAt: time })
  expect(first.read()).toHaveLength(1)
  expect(new ExportRecoveryStore(storage, 'http://127.0.0.1:18089/api/v1', id).read()).toEqual([])
  expect(
    new ExportRecoveryStore(storage, 'http://127.0.0.1:18088/api/v1', '2'.repeat(24)).read(),
  ).toEqual([])
  expect([...values.values()].join()).not.toMatch(/content|filter|token|path|password/i)
  expect(() => first.put({ requestId, kind: 'retry', jobId: id, createdAt: time })).toThrow()
  first.remove(requestId)
  expect(first.read()).toEqual([])
  first.put({ requestId, kind: 'retry', jobId: id, createdAt: time })
  const key = [...values.keys()][0]
  values.set(
    key,
    JSON.stringify({
      schemaVersion: 1,
      operations: [{ requestId, kind: 'retry', jobId: id, createdAt: time, token: 'forbidden' }],
    }),
  )
  expect(() => first.read()).toThrow()
  first.clear()
  expect(first.read()).toEqual([])
})
it('captures plain immutable request values even when source controls change later', () => {
  const source = {
    kind: 'resource' as const,
    resource: 'parameters',
    columns: [{ field: 'key', header: 'key' }],
    search: 'prefix',
    filters: [{ field: 'group', op: 'in' as const, value: ['a'] }],
    sort: null,
  }
  const request = exportInput(source, 'csv', 100000, requestId)
  source.columns[0].header = 'changed'
  source.filters[0].value.push('b')
  expect(request.source).toMatchObject({
    columns: [{ header: 'key' }],
    filters: [{ value: ['a'] }],
  })
  expect(() => exportInput(source, 'csv', 1000001, requestId)).toThrow()
})
