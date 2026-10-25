import { FILE_CHUNK_SIZE, hashBrowserFile, type FileSink, type UploadSource } from './files'
export const DATA_FILE_LIMIT = 6 * 1024 * 1024
export type DataFormat = 'csv' | 'xlsx'
export interface DataResource {
  key: string
  label: string
  fields: string[]
  importable: boolean
  readPermission: string
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
  key: string
  status: 'ready' | 'invalid' | 'duplicate' | 'conflict' | 'applied'
  error: string | null
  targetId: string | null
  values: Record<string, unknown>
}
export interface ImportView {
  id: string
  requestId: string
  resource: string
  expiresAt: number
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
      const chunk = await source.readChunk(index, signal)
      active(signal)
      if (
        !(chunk.bytes instanceof Blob) ||
        chunk.bytes.size !== Math.min(FILE_CHUNK_SIZE, size - index * FILE_CHUNK_SIZE)
      )
        throw new Error('源文件校验失败')
      chunks.push(new Uint8Array(await chunk.bytes.arrayBuffer()))
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
    !/^tri-admin-(parameters|users)\.(csv|xlsx)$/.test(data.name) ||
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
export function parseImportView(value: unknown): ImportView {
  const data = value as ImportView
  if (
    !data ||
    typeof data !== 'object' ||
    !/^[a-f0-9]{24}$/.test(data.id) ||
    typeof data.requestId !== 'string' ||
    !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(data.requestId) ||
    data.resource !== 'parameters' ||
    !Number.isSafeInteger(data.expiresAt) ||
    !Array.isArray(data.rows) ||
    data.rows.length > 2000
  )
    throw invalid()
  for (const [index, row] of data.rows.entries()) {
    if (
      !row ||
      row.index !== index ||
      typeof row.key !== 'string' ||
      row.key.length > 16384 ||
      !['ready', 'invalid', 'duplicate', 'conflict', 'applied'].includes(row.status) ||
      (row.error !== null && typeof row.error !== 'string') ||
      (row.targetId !== null &&
        (typeof row.targetId !== 'string' || !/^[a-f0-9]{24}$/.test(row.targetId))) ||
      !row.values ||
      typeof row.values !== 'object' ||
      Array.isArray(row.values) ||
      Object.keys(row.values).sort().join(',') !== 'description,group,key,name,value,valueType' ||
      !['name', 'key', 'group', 'description', 'valueType'].every(
        (k) => typeof row.values[k] === 'string',
      ) ||
      !safe(row.values.value)
    )
      throw invalid()
  }
  return data
}
