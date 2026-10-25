import { ApiError } from './client'
import { FILE_CHUNK_SIZE, validateFileSpec } from './files'
export interface FileCategory {
  id: string
  name: string
  code: string
  description: string
  version: number
  createdAt: string
  updatedAt: string
}
export interface StoredFile {
  id: string
  ownerId: string
  name: string
  mediaType: string
  size: number
  sha256: string
  categoryId: string | null
  storageProfileId: string
  version: number
  createdAt: string
  updatedAt: string
}
export interface UploadRequest {
  requestId: string
  name: string
  mediaType: string
  size: number
  sha256: string
  categoryId?: string
  storageProfileId?: string
}
export type UploadState =
  | 'receiving'
  | 'queued'
  | 'verifying'
  | 'publishing'
  | 'reconciling'
  | 'unknown'
  | 'failed'
  | 'cancelRequested'
  | 'cancelled'
  | 'completed'
export interface UploadRecord extends StoredFile {
  requestId: string
  state: UploadState
  chunkSize: number
  totalChunks: number
  receivedCount: number
  receivedBytes: number
  receivedIndices?: readonly number[]
  cleanupPending: boolean
  fileId: string | null
  lastError: string | null
  completedAt: string | null
}
export type UploadAction = 'complete' | 'retry' | 'cancel' | 'recover' | 'cleanup'
export const objectId = (value: unknown): value is string =>
  typeof value === 'string' && /^[a-f0-9]{24}$/.test(value)
export const requestId = (value: unknown): value is string =>
  typeof value === 'string' &&
  /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(value)
export function validFileName(name: unknown): name is string {
  if (
    typeof name !== 'string' ||
    !name ||
    name.trim() !== name ||
    new TextEncoder().encode(name).length > 255 ||
    name.endsWith('.') ||
    /[<>:"/\\|?*\x00-\x1f\x7f-\x9f]/.test(name) ||
    name === '.' ||
    name === '..'
  )
    return false
  return !/^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$/i.test(name.split('.')[0]!)
}
export function validMediaType(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    value.length <= 127 &&
    !/[\x00-\x1f\x7f-\x9f]/.test(value) &&
    /^[a-z0-9!#$%&'+.^_`|~-]+\/[a-z0-9!#$%&'+.^_`|~-]+$/i.test(value.split(';')[0]!.trim())
  )
}
export function validateUploadRequest(value: unknown): asserts value is UploadRequest {
  const row = value as UploadRequest | null
  const allowed = [
    'requestId',
    'name',
    'mediaType',
    'size',
    'sha256',
    'categoryId',
    'storageProfileId',
  ]
  if (
    !row ||
    typeof row !== 'object' ||
    !Object.keys(row).every((key) => allowed.includes(key)) ||
    !requestId(row.requestId) ||
    !validFileName(row.name) ||
    !validMediaType(row.mediaType) ||
    ('categoryId' in row && !objectId(row.categoryId)) ||
    ('storageProfileId' in row && !objectId(row.storageProfileId))
  )
    throw new ApiError('UPLOAD_DRAFT_INVALID', '上传名称、类型或选择信息无效')
  try {
    validateFileSpec(row)
  } catch {
    throw new ApiError('UPLOAD_DRAFT_INVALID', '文件大小或摘要无效')
  }
}
export function requestIdentity(row: UploadRequest): string {
  return JSON.stringify([
    row.requestId,
    row.name,
    row.mediaType,
    row.size,
    row.sha256,
    row.categoryId ?? null,
    row.storageProfileId ?? null,
  ])
}

export function parseUploadRecord(
  value: unknown,
  actorId: string,
  expected?: Readonly<UploadRequest>,
  detail = false,
): UploadRecord {
  const row = value as Partial<UploadRecord> | null
  const integer = (value: unknown, min: number, max: number): value is number =>
    typeof value === 'number' && Number.isSafeInteger(value) && value >= min && value <= max
  const date = (value: unknown): value is string =>
    typeof value === 'string' && value.length <= 64 && Number.isFinite(Date.parse(value))
  const states: UploadState[] = [
    'receiving',
    'queued',
    'verifying',
    'publishing',
    'reconciling',
    'unknown',
    'failed',
    'cancelRequested',
    'cancelled',
    'completed',
  ]
  const fail = () => new ApiError('UPLOAD_RESPONSE_INVALID', '上传响应与当前任务不一致，请刷新核对')
  if (
    !row ||
    !objectId(row.id) ||
    row.ownerId !== actorId ||
    !requestId(row.requestId) ||
    !validFileName(row.name) ||
    !validMediaType(row.mediaType) ||
    !objectId(row.storageProfileId) ||
    (row.categoryId !== null && !objectId(row.categoryId)) ||
    typeof row.state !== 'string' ||
    !states.includes(row.state as UploadState) ||
    row.chunkSize !== FILE_CHUNK_SIZE ||
    !integer(row.totalChunks, 0, 1280) ||
    !integer(row.receivedCount, 0, row.totalChunks) ||
    !integer(row.receivedBytes, 0, 5 * 1024 * 1024 * 1024) ||
    !integer(row.version, 1, Number.MAX_SAFE_INTEGER) ||
    typeof row.cleanupPending !== 'boolean' ||
    (row.fileId !== null && !objectId(row.fileId)) ||
    (row.lastError !== null &&
      (typeof row.lastError !== 'string' || !/^[A-Z0-9_]{1,80}$/.test(row.lastError))) ||
    !date(row.createdAt) ||
    !date(row.updatedAt) ||
    (row.completedAt !== null && !date(row.completedAt))
  )
    throw fail()
  try {
    validateFileSpec(row as UploadRecord)
  } catch {
    throw fail()
  }
  if (
    row.totalChunks !== Math.ceil(row.size! / FILE_CHUNK_SIZE) ||
    row.receivedBytes > row.size! ||
    (row.state === 'completed' && row.fileId !== row.id)
  )
    throw fail()
  let indices: number[] | undefined
  if (row.receivedIndices !== undefined) {
    if (!Array.isArray(row.receivedIndices) || row.receivedIndices.length !== row.receivedCount)
      throw fail()
    indices = [...row.receivedIndices]
    if (
      indices.some(
        (index, n) =>
          !integer(index, 0, row.totalChunks! - 1) || (n > 0 && index <= indices![n - 1]!),
      ) ||
      indices.reduce(
        (sum, index) => sum + Math.min(FILE_CHUNK_SIZE, row.size! - index * FILE_CHUNK_SIZE),
        0,
      ) !== row.receivedBytes
    )
      throw fail()
  } else if (detail) throw fail()
  if (
    expected &&
    (row.requestId !== expected.requestId ||
      row.name !== expected.name ||
      row.size !== expected.size ||
      row.sha256 !== expected.sha256 ||
      row.categoryId !== (expected.categoryId ?? null) ||
      (expected.storageProfileId !== undefined &&
        row.storageProfileId !== expected.storageProfileId))
  )
    throw fail()
  return Object.freeze({
    id: row.id,
    ownerId: actorId,
    requestId: row.requestId,
    name: row.name,
    mediaType: row.mediaType,
    size: row.size!,
    sha256: row.sha256!,
    categoryId: row.categoryId,
    storageProfileId: row.storageProfileId,
    state: row.state as UploadState,
    chunkSize: row.chunkSize,
    totalChunks: row.totalChunks,
    receivedCount: row.receivedCount,
    receivedBytes: row.receivedBytes,
    ...(indices ? { receivedIndices: Object.freeze(indices) } : {}),
    cleanupPending: row.cleanupPending,
    fileId: row.fileId,
    lastError: row.lastError,
    completedAt: row.completedAt,
    version: row.version,
    createdAt: row.createdAt,
    updatedAt: row.updatedAt,
  })
}
