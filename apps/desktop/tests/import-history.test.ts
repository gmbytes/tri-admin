import { it, expect } from 'vitest'
import { parseImportSummary, parseHistoryPage, parseImportView } from '../src/core/data-tools'
function summary() {
  return {
    id: '1'.repeat(24),
    requestId: crypto.randomUUID(),
    resource: 'parameters',
    createdAt: new Date(0).toISOString(),
    expiresAt: new Date(Date.now() + 3600000).toISOString(),
    rowCount: 2,
    readyCount: 0,
    appliedCount: 1,
    errorCount: 1,
    expiredCount: 1,
    status: 'partial',
    payloadState: 'redacted',
    redactedAt: new Date().toISOString(),
    template: null,
  }
}
it('validates paged receipt summaries without accepting inconsistent counters or hidden row values', () => {
  const data = summary()
  expect(parseImportSummary(data).status).toBe('partial')
  for (const change of [
    { readyCount: 1 },
    { rowCount: 3 },
    { expiredCount: 2 },
    { payloadState: 'unknown' },
    { status: 'completed' },
    { rows: [] },
  ])
    expect(() => parseImportSummary({ ...data, ...change })).toThrow()
  const page = parseHistoryPage(
    { data: [data], meta: { page: 2, pageSize: 20, total: 21 } },
    { page: 2, pageSize: 20 },
  )
  expect(page.rows).toHaveLength(1)
  expect(page.total).toBe(21)
  expect(() =>
    parseHistoryPage(
      { data: [data], meta: { page: 1, pageSize: 20, total: 21 } },
      { page: 2, pageSize: 20 },
    ),
  ).toThrow()
})
it('keeps applied targets while refusing leaked values or ready rows in unavailable payloads', () => {
  const data = {
    ...summary(),
    rows: [
      {
        index: 0,
        key: null,
        status: 'applied',
        error: null,
        targetId: '2'.repeat(24),
        values: null,
      },
      {
        index: 1,
        key: null,
        status: 'expired',
        error: 'PREVIEW_EXPIRED',
        targetId: null,
        values: null,
      },
    ],
  }
  expect(parseImportView(data).rows[0].targetId).toBe('2'.repeat(24))
  expect(() =>
    parseImportView({
      ...data,
      rows: [{ ...data.rows[0], values: { value: 'private' } }, data.rows[1]],
    }),
  ).toThrow()
  expect(() =>
    parseImportView({ ...data, rows: [data.rows[0], { ...data.rows[1], status: 'ready' }] }),
  ).toThrow()
  expect(() =>
    parseImportView({ ...data, rows: [{ ...data.rows[0], targetId: null }, data.rows[1]] }),
  ).toThrow()
})
