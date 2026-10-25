import {
  FILE_CHUNK_SIZE,
  hashBrowserFile,
  cancellable,
  type FileSink,
  type UploadSource,
} from './files'
export const DATA_FILE_LIMIT = 6 * 1024 * 1024
export type DataFormat = 'csv' | 'xlsx'
export interface DataResource {
  key: string
  label: string
  fields: string[]
  importable: boolean
  readPermission: string
  writePermission: string | null
  requiredFields: string[]
}
export interface DataDownload {
  readonly name: string
  readonly mimeType: string
  readonly size: number
  readonly sha256: string
  readonly contentBase64: string
  readonly rowCount: number
}
export interface ImportRow {
  index: number
  key: string | null
  status: 'ready' | 'invalid' | 'duplicate' | 'conflict' | 'applied' | 'expired'
  error: string | null
  targetId: string | null
  values: Record<string, unknown> | null
}
export interface ImportSummary {
  id: string
  requestId: string
  resource: string
  expiresAt: string
  createdAt: string
  rowCount: number
  readyCount: number
  appliedCount: number
  errorCount: number
  expiredCount: number
  status: 'pending' | 'completed' | 'partial' | 'failed' | 'expired'
  payloadState: 'full' | 'expired' | 'redacted'
  redactedAt: string | null
  template: { id: string; name: string; code: string; version: number } | null
}
export interface ImportView extends ImportSummary {
  rows: ImportRow[]
}
function invalid() {
  return new Error('数据工具响应格式或大小无效')
}
function active(signal?: AbortSignal) {
  if (signal?.aborted) throw new Error('文件操作已取消')
}
function validBase64(text: string): boolean {
  if (text.length % 4 !== 0) return false
  let end = text.length
  while (end > 0 && text.charCodeAt(end - 1) === 61) end--
  if (text.length - end > 2) return false
  for (let i = 0; i < end; i++) {
    const c = text.charCodeAt(i)
    if (!(
      (c >= 65 && c <= 90) ||
      (c >= 97 && c <= 122) ||
      (c >= 48 && c <= 57) ||
      c === 43 ||
      c === 47
    ))
      return false
  }
  return true
}
function base64(bytes: Uint8Array) {
  let text = ''
  for (let offset = 0; offset < bytes.length; offset += 64 * 1024)
    text += String.fromCharCode(...bytes.subarray(offset, offset + 64 * 1024))
  return btoa(text)
}
export async function dataSourceBase64(
  source: UploadSource,
  signal?: AbortSignal,
): Promise<string> {
  const size = source.size,
    sha256 = source.sha256
  try {
    active(signal)
    if (!Number.isSafeInteger(size) || size < 0 || size > DATA_FILE_LIMIT)
      throw new Error('数据文件最多 6 MiB')
    const chunks: Uint8Array<ArrayBuffer>[] = []
    for (let index = 0; index < Math.ceil(size / FILE_CHUNK_SIZE); index++) {
      const chunk = await cancellable(() => source.readChunk(index, signal), signal)
      active(signal)
      if (
        !(chunk.bytes instanceof Blob) ||
        chunk.bytes.size !== Math.min(FILE_CHUNK_SIZE, size - index * FILE_CHUNK_SIZE)
      )
        throw new Error('源文件校验失败')
      chunks.push(new Uint8Array(await cancellable(() => chunk.bytes.arrayBuffer(), signal)))
      active(signal)
    }
    const content = new Blob(chunks)
    if (content.size !== size || (await hashBrowserFile(content, signal)) !== sha256)
      throw new Error('源文件校验失败')
    const bytes = new Uint8Array(await content.arrayBuffer())
    active(signal)
    return base64(bytes)
  } finally {
    await source.close()
  }
}
export function parseDataDownload(value: unknown): DataDownload {
  const data = value as DataDownload
  if (
    !data ||
    typeof data !== 'object' ||
    typeof data.name !== 'string' ||
    !/^tri-admin-(parameters|users|dictionaries|dictionaryItems)\.(csv|xlsx)$/.test(data.name) ||
    !Number.isSafeInteger(data.size) ||
    data.size < 0 ||
    data.size > DATA_FILE_LIMIT ||
    typeof data.sha256 !== 'string' ||
    !/^[a-f0-9]{64}$/.test(data.sha256) ||
    typeof data.contentBase64 !== 'string' ||
    data.contentBase64.length > Math.ceil(DATA_FILE_LIMIT / 3) * 4 ||
    !validBase64(data.contentBase64) ||
    !Number.isSafeInteger(data.rowCount) ||
    data.rowCount < 0 ||
    data.rowCount > 2000 ||
    data.mimeType !==
      (data.name.endsWith('.csv')
        ? 'text/csv;charset=utf-8'
        : 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet')
  )
    throw invalid()
  return Object.freeze({
    name: data.name,
    mimeType: data.mimeType,
    size: data.size,
    sha256: data.sha256,
    contentBase64: data.contentBase64,
    rowCount: data.rowCount,
  })
}
export async function saveDataDownload<T>(
  download: DataDownload,
  sink: FileSink<T>,
  signal?: AbortSignal,
): Promise<T> {
  try {
    active(signal)
    const data = parseDataDownload(download),
      binary = atob(data.contentBase64)
    if (binary.length !== data.size) throw new Error('下载内容校验失败')
    const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0))
    if ((await hashBrowserFile(new Blob([bytes]), signal)) !== data.sha256)
      throw new Error('下载内容校验失败')
    for (let offset = 0; offset < bytes.length; offset += 64 * 1024) {
      active(signal)
      await sink.write(bytes.subarray(offset, offset + 64 * 1024))
    }
    active(signal)
    return await sink.complete({ size: data.size, sha256: data.sha256 })
  } catch (error) {
    await sink.abort()
    throw error
  }
}
function safe(value: unknown, depth = 0): boolean {
  if (depth > 16) return false
  if (typeof value === 'number')
    return Number.isFinite(value) && Math.abs(value) <= Number.MAX_SAFE_INTEGER
  if (Array.isArray(value)) return value.every((v) => safe(v, depth + 1))
  if (value && typeof value === 'object')
    return Object.values(value).every((v) => safe(v, depth + 1))
  return true
}
function timestamp(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    Number.isFinite(Date.parse(value)) &&
    new Date(value).toISOString() === value
  )
}
export function parseImportSummary(value: unknown): ImportSummary {
  const data = value as ImportSummary
  if (
    !data ||
    typeof data !== 'object' ||
    Object.keys(data).sort().join(',') !==
      'appliedCount,createdAt,errorCount,expiredCount,expiresAt,id,payloadState,readyCount,redactedAt,requestId,resource,rowCount,status,template' ||
    typeof data.id !== 'string' ||
    !/^[a-f0-9]{24}$/.test(data.id) ||
    typeof data.requestId !== 'string' ||
    !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(data.requestId) ||
    !['parameters', 'dictionaries', 'dictionaryItems'].includes(data.resource) ||
    !timestamp(data.createdAt) ||
    !timestamp(data.expiresAt) ||
    !['full', 'expired', 'redacted'].includes(data.payloadState) ||
    ![data.rowCount, data.readyCount, data.appliedCount, data.errorCount, data.expiredCount].every(
      (v) => Number.isSafeInteger(v) && v >= 0 && v <= 2000,
    ) ||
    data.readyCount + data.appliedCount + data.errorCount !== data.rowCount ||
    data.expiredCount > data.errorCount ||
    (data.payloadState !== 'full' && data.readyCount !== 0) ||
    (data.payloadState === 'redacted' ? !timestamp(data.redactedAt) : data.redactedAt !== null)
  )
    throw invalid()
  if (
    data.template !== null &&
    (!data.template ||
      Object.keys(data.template).sort().join(',') !== 'code,id,name,version' ||
      typeof data.template.id !== 'string' ||
      !/^[a-f0-9]{24}$/.test(data.template.id) ||
      typeof data.template.name !== 'string' ||
      Array.from(data.template.name).length > 80 ||
      typeof data.template.code !== 'string' ||
      !/^[a-z][a-z0-9._-]{0,63}$/.test(data.template.code) ||
      !Number.isSafeInteger(data.template.version) ||
      data.template.version < 1)
  )
    throw invalid()
  const status =
    data.readyCount > 0
      ? 'pending'
      : data.expiredCount > 0 && data.appliedCount === 0
        ? 'expired'
        : data.errorCount > 0
          ? data.appliedCount > 0
            ? 'partial'
            : 'failed'
          : 'completed'
  if (data.status !== status) throw invalid()
  return data
}
export function parseHistoryPage(
  value: unknown,
  expected: { page: number; pageSize: number },
): { rows: ImportSummary[]; total: number } {
  const response = value as {
    data: unknown[]
    meta: { page: number; pageSize: number; total: number }
  }
  if (
    !response ||
    !Array.isArray(response.data) ||
    !response.meta ||
    response.meta.page !== expected.page ||
    response.meta.pageSize !== expected.pageSize ||
    !Number.isSafeInteger(response.meta.total) ||
    response.meta.total < response.data.length ||
    response.data.length > expected.pageSize
  )
    throw invalid()
  return { rows: response.data.map(parseImportSummary), total: response.meta.total }
}
export function parseImportView(value: unknown): ImportView {
  if (!value || typeof value !== 'object') throw invalid()
  const data = value as ImportView,
    { rows, ...metadata } = data
  const summary = parseImportSummary(metadata)
  if (!Array.isArray(rows) || rows.length !== summary.rowCount) throw invalid()
  const shape =
    data.resource === 'parameters'
      ? {
          keys: 'description,group,key,name,value,valueType',
          strings: ['name', 'key', 'group', 'description', 'valueType'],
        }
      : data.resource === 'dictionaries'
        ? { keys: 'description,enabled,key,name', strings: ['name', 'key', 'description'] }
        : {
            keys: 'dictionaryKey,enabled,label,sort,value',
            strings: ['dictionaryKey', 'label', 'value'],
          }
  let ready = 0,
    applied = 0,
    errors = 0,
    expired = 0
  for (const [index, row] of rows.entries()) {
    if (
      !row ||
      row.index !== index ||
      !['ready', 'invalid', 'duplicate', 'conflict', 'applied', 'expired'].includes(row.status) ||
      (row.error !== null && (typeof row.error !== 'string' || row.error.length > 128)) ||
      (row.targetId !== null &&
        (typeof row.targetId !== 'string' || !/^[a-f0-9]{24}$/.test(row.targetId))) ||
      (row.status === 'applied'
        ? row.targetId === null || row.error !== null
        : row.targetId !== null)
    )
      throw invalid()
    if (summary.payloadState === 'full') {
      if (
        typeof row.key !== 'string' ||
        row.key.length > 32772 ||
        !row.values ||
        typeof row.values !== 'object' ||
        Array.isArray(row.values) ||
        Object.keys(row.values).sort().join(',') !== shape.keys ||
        !shape.strings.every((k) => typeof row.values![k] === 'string') ||
        !(data.resource === 'parameters' ? safe(row.values.value) : safe(row.values)) ||
        (data.resource !== 'parameters' &&
          !['boolean', 'string'].includes(typeof row.values.enabled)) ||
        (data.resource === 'dictionaryItems' &&
          !['number', 'string'].includes(typeof row.values.sort))
      )
        throw invalid()
    } else if (row.key !== null || row.values !== null || row.status === 'ready') throw invalid()
    if (row.status === 'ready') ready++
    else if (row.status === 'applied') applied++
    else {
      errors++
      if (row.status === 'expired') expired++
    }
  }
  if (
    ready !== summary.readyCount ||
    applied !== summary.appliedCount ||
    errors !== summary.errorCount ||
    expired !== summary.expiredCount
  )
    throw invalid()
  return data
}
