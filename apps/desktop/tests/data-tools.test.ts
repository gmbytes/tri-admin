import { expect, it, vi } from 'vitest'
import { hashBrowserFile, type UploadSource } from '../src/core/files'
import {
  dataSourceBase64,
  parseDataDownload,
  saveDataDownload,
  parseImportView,
  DATA_FILE_LIMIT,
} from '../src/core/data-tools'

it('reads bounded source chunks, verifies whole content and always closes the handle', async () => {
  const bytes = new Blob(['中文 data']),
    sha256 = await hashBrowserFile(bytes)
  const source: UploadSource = {
    name: 'data.csv',
    mediaType: 'text/csv',
    size: bytes.size,
    sha256,
    readChunk: vi.fn(async () => ({ bytes, sha256 })),
    close: vi.fn(async () => {}),
  }
  expect(atob(await dataSourceBase64(source))).toBe(
    String.fromCharCode(...new Uint8Array(await bytes.arrayBuffer())),
  )
  expect(source.close).toHaveBeenCalledOnce()
  const oversized = { ...source, size: DATA_FILE_LIMIT + 1, close: vi.fn(async () => {}) }
  await expect(dataSourceBase64(oversized)).rejects.toThrow('6 MiB')
  expect(oversized.close).toHaveBeenCalledOnce()
})
it('rejects changed source bytes and cancelled reads before preparing a preview', async () => {
  const sha256 = await hashBrowserFile(new Blob(['original']))
  const source: UploadSource = {
    name: 'data.csv',
    mediaType: 'text/csv',
    size: 8,
    sha256,
    readChunk: async () => ({ bytes: new Blob(['modified']), sha256 }),
    close: vi.fn(async () => {}),
  }
  await expect(dataSourceBase64(source)).rejects.toThrow('校验')
  const control = new AbortController()
  control.abort()
  await expect(dataSourceBase64(source, control.signal)).rejects.toThrow('取消')
  expect(source.close).toHaveBeenCalledTimes(2)
})
it('validates download metadata, checks bytes before commit and aborts failed saves', async () => {
  const content = 'a,b\n1,2\n',
    sha256 = await hashBrowserFile(new Blob([content]))
  const value = {
    name: 'tri-admin-parameters.csv',
    mimeType: 'text/csv;charset=utf-8',
    size: content.length,
    sha256,
    contentBase64: btoa(content),
    rowCount: 1,
  }
  const download = parseDataDownload(value)
  const sink = {
    write: vi.fn(async () => {}),
    complete: vi.fn(async () => 'saved'),
    abort: vi.fn(async () => {}),
  }
  expect(await saveDataDownload(download, sink)).toBe('saved')
  expect(sink.complete).toHaveBeenCalledOnce()
  const bad = parseDataDownload({ ...value, sha256: '0'.repeat(64) })
  await expect(saveDataDownload(bad, sink)).rejects.toThrow('校验')
  expect(sink.abort).toHaveBeenCalledOnce()
  for (const change of [
    { size: DATA_FILE_LIMIT + 1 },
    { name: '../data.csv' },
    { contentBase64: 'not base64' },
    { rowCount: 2001 },
  ])
    expect(() => parseDataDownload({ ...value, ...change })).toThrow()
})
it('requires bounded owner-safe import result shape and known row states', () => {
  const row = {
    index: 0,
    key: 'test.key',
    status: 'ready',
    error: null,
    targetId: null,
    values: {
      name: 'Test',
      key: 'test.key',
      group: 'general',
      description: '',
      valueType: 'string',
      value: 'x',
    },
  }
  const view = {
    id: '1'.repeat(24),
    requestId: crypto.randomUUID(),
    resource: 'parameters',
    expiresAt: new Date(Date.now() + 3600000).toISOString(),
    rows: [row],
  }
  expect(parseImportView(view).rows[0].status).toBe('ready')
  for (const changed of [
    { ...view, rows: [{ ...row, index: 2 }] },
    { ...view, resource: 'customers' },
    { ...view, rows: [{ ...row, status: 'unknown' }] },
    { ...view, rows: [{ ...row, values: { password: 'secret' } }] },
  ])
    expect(() => parseImportView(changed)).toThrow()
})
it('accepts a maximum-size download without recursive base64 matching', () => {
  const bytes = new Uint8Array(DATA_FILE_LIMIT),
    parts: string[] = []
  for (let i = 0; i < bytes.length; i += 65536)
    parts.push(String.fromCharCode(...bytes.subarray(i, i + 65536)))
  const value = {
    name: 'tri-admin-parameters.csv',
    mimeType: 'text/csv;charset=utf-8',
    size: bytes.length,
    sha256: '0'.repeat(64),
    contentBase64: btoa(parts.join('')),
    rowCount: 2000,
  }
  expect(parseDataDownload(value).size).toBe(DATA_FILE_LIMIT)
})
it('accepts the domain JSON depth boundary without counting the preview wrapper', () => {
  let value: unknown = null
  for (let i = 0; i < 16; i++) value = [value]
  const row = {
    index: 0,
    key: 'nested',
    status: 'ready',
    error: null,
    targetId: null,
    values: {
      name: 'Nested',
      key: 'nested',
      group: 'general',
      description: '',
      valueType: 'json',
      value,
    },
  }
  const view = {
    id: '1'.repeat(24),
    requestId: crypto.randomUUID(),
    resource: 'parameters',
    expiresAt: new Date(Date.now() + 3600000).toISOString(),
    rows: [row],
  }
  expect(parseImportView(view).rows[0].values.value).toEqual(value)
  expect(() =>
    parseImportView({ ...view, rows: [{ ...row, values: { ...row.values, value: [value] } }] }),
  ).toThrow()
})
it('validates dictionary and item results without accepting extra credential fields', () => {
  for (const [resource, values] of Object.entries({
    dictionaries: { name: 'Dictionary', key: 'dictionary', description: '', enabled: false },
    dictionaryItems: {
      dictionaryKey: 'dictionary',
      label: 'Item',
      value: '001',
      sort: -3,
      enabled: true,
    },
  })) {
    const view = {
      id: '1'.repeat(24),
      requestId: crypto.randomUUID(),
      resource,
      expiresAt: new Date(Date.now() + 3600000).toISOString(),
      rows: [
        { index: 0, key: 'dictionary / 001', status: 'ready', error: null, targetId: null, values },
      ],
    }
    expect(parseImportView(view).resource).toBe(resource)
    expect(() =>
      parseImportView({
        ...view,
        rows: [{ ...view.rows[0], values: { ...values, password: 'secret' } }],
      }),
    ).toThrow()
    expect(
      parseDataDownload({
        name: `tri-admin-${resource}.csv`,
        mimeType: 'text/csv;charset=utf-8',
        size: 0,
        sha256: '0'.repeat(64),
        contentBase64: '',
        rowCount: 0,
      }).name,
    ).toContain(resource)
  }
})
it('uses canonical RFC3339 expiry text for the public import contract', () => {
  const value = {
    id: '1'.repeat(24),
    requestId: crypto.randomUUID(),
    resource: 'parameters',
    expiresAt: new Date(Date.now() + 3600000).toISOString(),
    rows: [],
  }
  expect(parseImportView(value).expiresAt).toBe(value.expiresAt)
  expect(() => parseImportView({ ...value, expiresAt: Date.now() })).toThrow()
})
