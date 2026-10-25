import type { DataFormat } from './data-tools'
import type { DataFilter, DataSort, TemplateColumn } from './export-templates'
import { normalizeEndpoint } from './endpoint'
export const exportStates = [
  'queued',
  'running',
  'publishing',
  'reconciling',
  'unknown',
  'completed',
  'failed',
  'cancelled',
  'expired',
] as const
export type ExportState = (typeof exportStates)[number]
export type ExportSource =
  | {
      kind: 'resource'
      resource: string
      columns: TemplateColumn[]
      search: string
      filters: DataFilter[]
      sort: DataSort | null
    }
  | { kind: 'template'; id: string; version: number }
export interface ExportFile {
  name: string
  mimeType: string
  size: number
  sha256: string
}
export interface ExportJob {
  id: string
  requestId: string
  resource: string
  format: DataFormat
  maxRows: number
  template: { id: string; name: string; code: string; version: number } | null
  state: ExportState
  rowCount: number
  file: ExportFile | null
  lastError: string | null
  cleanupPending: boolean
  version: number
  createdAt: string
  updatedAt: string
  startedAt: string | null
  completedAt: string | null
  expiresAt: string | null
}
export interface ExportAction {
  jobId: string
  requestId: string
  kind: 'retry'
  generation: number
  createdAt: string
}
export interface ExportOperation {
  requestId: string
  kind: 'create' | 'retry' | 'cancel'
  jobId: string | null
  createdAt: string
}
type ObjectValue = Record<string, unknown>
const resources = ['parameters', 'users', 'dictionaries', 'dictionaryItems'],
  oid = /^[a-f0-9]{24}$/,
  uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/
function invalid() {
  return new Error('导出任务响应或本机恢复记录无效')
}
function object(value: unknown): value is ObjectValue {
  return !!value && typeof value === 'object' && !Array.isArray(value)
}
function exact(value: ObjectValue, keys: string[]) {
  return Object.keys(value).length === keys.length && keys.every((key) => Object.hasOwn(value, key))
}
function date(value: unknown): value is string {
  if (typeof value !== 'string' || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/.test(value))
    return false
  try {
    return new Date(value).toISOString() === value
  } catch {
    return false
  }
}
function integer(value: unknown, min: number, max = Number.MAX_SAFE_INTEGER): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= min && value <= max
}
function objectId(value: unknown): value is string {
  return typeof value === 'string' && oid.test(value)
}
export function validExportRequest(value: unknown): value is string {
  return typeof value === 'string' && uuid.test(value)
}
export function parseExportJob(value: unknown): ExportJob {
  if (
    !object(value) ||
    !exact(value, [
      'id',
      'requestId',
      'resource',
      'format',
      'maxRows',
      'template',
      'state',
      'rowCount',
      'file',
      'lastError',
      'cleanupPending',
      'version',
      'createdAt',
      'updatedAt',
      'startedAt',
      'completedAt',
      'expiresAt',
    ])
  )
    throw invalid()
  const data = value as unknown as ExportJob
  if (
    !objectId(data.id) ||
    !validExportRequest(data.requestId) ||
    !resources.includes(data.resource) ||
    !['csv', 'xlsx'].includes(data.format) ||
    !integer(data.maxRows, 1, 1000000) ||
    !integer(data.rowCount, 0, data.maxRows) ||
    !exportStates.includes(data.state) ||
    !integer(data.version, 1) ||
    typeof data.cleanupPending !== 'boolean' ||
    !date(data.createdAt) ||
    !date(data.updatedAt) ||
    [data.startedAt, data.completedAt, data.expiresAt].some(
      (value) => value !== null && !date(value),
    ) ||
    (data.lastError !== null &&
      (typeof data.lastError !== 'string' || !/^[A-Z][A-Z0-9_]{0,79}$/.test(data.lastError)))
  )
    throw invalid()
  if (
    data.template !== null &&
    (!object(data.template) ||
      !exact(data.template, ['id', 'name', 'code', 'version']) ||
      !objectId(data.template.id) ||
      typeof data.template.name !== 'string' ||
      !data.template.name.trim() ||
      Array.from(data.template.name).length > 80 ||
      typeof data.template.code !== 'string' ||
      !/^[a-z][a-z0-9._-]{0,63}$/.test(data.template.code) ||
      !integer(data.template.version, 1))
  )
    throw invalid()
  if (data.file !== null) {
    if (
      data.state !== 'completed' ||
      !object(data.file) ||
      !exact(data.file, ['name', 'mimeType', 'size', 'sha256']) ||
      data.file.name !== `tri-admin-${data.resource}-${data.id}.${data.format}` ||
      !integer(data.file.size, 0, 5 * 1024 ** 3) ||
      typeof data.file.sha256 !== 'string' ||
      !/^[a-f0-9]{64}$/.test(data.file.sha256) ||
      data.file.mimeType !==
        (data.format === 'csv'
          ? 'text/csv;charset=utf-8'
          : 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet')
    )
      throw invalid()
  }
  if (data.state === 'completed' && (!data.file || !data.completedAt || !data.expiresAt))
    throw invalid()
  return data
}
export function parseExportPage(
  value: unknown,
  expected: { page: number; pageSize: number },
): { rows: ExportJob[]; total: number } {
  if (
    !object(value) ||
    !exact(value, ['data', 'meta']) ||
    !Array.isArray(value.data) ||
    value.data.length > expected.pageSize ||
    !object(value.meta) ||
    !exact(value.meta, ['page', 'pageSize', 'total']) ||
    value.meta.page !== expected.page ||
    value.meta.pageSize !== expected.pageSize ||
    !integer(value.meta.total, 0) ||
    value.meta.total < value.data.length
  )
    throw invalid()
  return { rows: value.data.map(parseExportJob), total: value.meta.total }
}
export function parseExportAction(value: unknown): ExportAction {
  if (!object(value) || !exact(value, ['jobId', 'requestId', 'kind', 'generation', 'createdAt']))
    throw invalid()
  const data = value as unknown as ExportAction
  if (
    !objectId(data.jobId) ||
    !validExportRequest(data.requestId) ||
    data.kind !== 'retry' ||
    !integer(data.generation, 0) ||
    !date(data.createdAt)
  )
    throw invalid()
  return data
}
export function exportInput(
  source: ExportSource,
  format: DataFormat,
  maxRows: number,
  requestId: string,
) {
  if (
    !validExportRequest(requestId) ||
    !['csv', 'xlsx'].includes(format) ||
    !integer(maxRows, 1, 1000000) ||
    !object(source)
  )
    throw invalid()
  if (source.kind === 'template') {
    if (!objectId(source.id) || !integer(source.version, 1)) throw invalid()
  } else if (
    source.kind !== 'resource' ||
    !resources.includes(source.resource) ||
    !source.columns.length ||
    source.columns.length > 32 ||
    source.search.length > 100 ||
    source.filters.length > 8
  )
    throw invalid()
  return { requestId, format, maxRows, source: JSON.parse(JSON.stringify(source)) as ExportSource }
}
function operation(value: unknown): ExportOperation {
  if (!object(value) || !exact(value, ['requestId', 'kind', 'jobId', 'createdAt'])) throw invalid()
  const data = value as unknown as ExportOperation
  if (
    !validExportRequest(data.requestId) ||
    !['create', 'retry', 'cancel'].includes(data.kind) ||
    !date(data.createdAt) ||
    (data.kind === 'create' ? data.jobId !== null : !objectId(data.jobId))
  )
    throw invalid()
  return data
}
export class ExportRecoveryStore {
  private key: string
  constructor(
    private storage: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>,
    endpoint: string,
    actor: string,
  ) {
    if (!objectId(actor)) throw invalid()
    this.key = `tri-admin.export-operations.v1:${encodeURIComponent(normalizeEndpoint(endpoint))}:${actor}`
  }
  read(): ExportOperation[] {
    const raw = this.storage.getItem(this.key)
    if (raw === null) return []
    if (raw.length > 16384) throw invalid()
    let data: unknown
    try {
      data = JSON.parse(raw)
    } catch {
      throw invalid()
    }
    if (
      !object(data) ||
      !exact(data, ['schemaVersion', 'operations']) ||
      data.schemaVersion !== 1 ||
      !Array.isArray(data.operations) ||
      data.operations.length > 16
    )
      throw invalid()
    const operations = data.operations.map(operation)
    if (new Set(operations.map((value) => value.requestId)).size !== operations.length)
      throw invalid()
    return operations
  }
  put(value: ExportOperation) {
    operation(value)
    const records = this.read()
    const previous = records.find((row) => row.requestId === value.requestId)
    if (previous && (previous.kind !== value.kind || previous.jobId !== value.jobId))
      throw invalid()
    const next = records.filter((row) => row.requestId !== value.requestId)
    next.push({ ...value })
    if (next.length > 16) throw new Error('最多保留 16 个待确认操作，请先查询已有操作')
    this.storage.setItem(this.key, JSON.stringify({ schemaVersion: 1, operations: next }))
  }
  remove(requestId: string) {
    const records = this.read().filter((row) => row.requestId !== requestId)
    if (records.length)
      this.storage.setItem(this.key, JSON.stringify({ schemaVersion: 1, operations: records }))
    else this.storage.removeItem(this.key)
  }
  clear() {
    this.storage.removeItem(this.key)
  }
}
