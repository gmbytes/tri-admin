import { ApiError } from './client'
import { normalizeEndpoint } from './endpoint'
import {
  objectId,
  requestId,
  requestIdentity,
  validateUploadRequest,
  type UploadAction,
  type UploadRequest,
} from './file-models'
export interface UploadJournalStorage {
  readonly length: number
  key(index: number): string | null
  getItem(key: string): string | null
  setItem(key: string, value: string): void
  removeItem(key: string): void
}
export interface UploadScope {
  endpoint: string
  actorId: string
}
export interface UploadDraft {
  readonly schemaVersion: 1
  readonly createdAt: string
  readonly request: Readonly<UploadRequest>
  readonly uploadId: string | null
  readonly pendingAction: Readonly<{
    actionId: string
    kind: UploadAction
    requestedAt: string
  }> | null
}
const actions = ['complete', 'retry', 'cancel', 'recover', 'cleanup']
function invalid() {
  return new ApiError('UPLOAD_JOURNAL_INVALID', '本机续传记录无效，请核对上传任务后处理')
}
function unavailable() {
  return new ApiError(
    'UPLOAD_JOURNAL_UNAVAILABLE',
    '无法保存本机续传记录，请检查存储空间或浏览器设置',
  )
}
function timestamp(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    value.length <= 32 &&
    Number.isFinite(Date.parse(value)) &&
    new Date(value).toISOString() === value
  )
}
function snapshot(value: unknown): UploadDraft {
  const row = value as UploadDraft | null
  if (
    !row ||
    typeof row !== 'object' ||
    Object.keys(row).sort().join(',') !==
      'createdAt,pendingAction,request,schemaVersion,uploadId' ||
    row.schemaVersion !== 1 ||
    !timestamp(row.createdAt) ||
    (row.uploadId !== null && !objectId(row.uploadId))
  )
    throw invalid()
  try {
    validateUploadRequest(row.request)
  } catch {
    throw invalid()
  }
  if (
    row.pendingAction !== null &&
    (typeof row.pendingAction !== 'object' ||
      Object.keys(row.pendingAction).sort().join(',') !== 'actionId,kind,requestedAt' ||
      !requestId(row.pendingAction.actionId) ||
      !actions.includes(row.pendingAction.kind) ||
      !timestamp(row.pendingAction.requestedAt))
  )
    throw invalid()
  return Object.freeze({
    ...row,
    request: Object.freeze({ ...row.request }),
    pendingAction: row.pendingAction ? Object.freeze({ ...row.pendingAction }) : null,
  })
}
export class UploadJournal {
  readonly scope: Readonly<UploadScope>
  private prefix: string
  constructor(
    private storage: UploadJournalStorage,
    scope: UploadScope,
  ) {
    try {
      if (!objectId(scope.actorId)) throw Error()
      this.scope = Object.freeze({
        endpoint: normalizeEndpoint(scope.endpoint),
        actorId: scope.actorId,
      })
    } catch {
      throw new ApiError('UPLOAD_SCOPE_INVALID', '续传记录的服务器或账号无效')
    }
    this.prefix = `tri-admin.uploads.v1:${encodeURIComponent(this.scope.endpoint)}:${this.scope.actorId}:`
  }
  list(): UploadDraft[] {
    try {
      const result: UploadDraft[] = []
      for (let n = 0; n < this.storage.length; n++) {
        const key = this.storage.key(n)
        if (!key?.startsWith(this.prefix)) continue
        const text = this.storage.getItem(key)
        if (text === null) continue
        if (text.length > 4096) throw invalid()
        let value: unknown
        try {
          value = JSON.parse(text)
        } catch {
          throw invalid()
        }
        const row = snapshot(value)
        if (key !== this.prefix + row.request.requestId) throw invalid()
        result.push(row)
        if (result.length > 32) throw invalid()
      }
      return result.sort(
        (a, b) =>
          a.createdAt.localeCompare(b.createdAt) ||
          a.request.requestId.localeCompare(b.request.requestId),
      )
    } catch (error) {
      if (error instanceof ApiError) throw error
      throw unavailable()
    }
  }
  get(id: string): UploadDraft {
    const row = this.list().find((row) => row.request.requestId === id)
    if (!row) throw new ApiError('UPLOAD_DRAFT_MISSING', '本机续传记录不存在')
    return row
  }
  private save(row: UploadDraft): UploadDraft {
    const value = snapshot(row)
    try {
      this.storage.setItem(this.prefix + value.request.requestId, JSON.stringify(value))
    } catch {
      throw unavailable()
    }
    return value
  }
  create(input: UploadRequest): UploadDraft {
    validateUploadRequest(input)
    const rows = this.list(),
      existing = rows.find((row) => row.request.requestId === input.requestId)
    if (existing) {
      if (requestIdentity(existing.request) !== requestIdentity(input))
        throw new ApiError('UPLOAD_DRAFT_CONFLICT', '不能修改待确认上传的原始条件')
      return existing
    }
    if (rows.length >= 32)
      throw new ApiError('UPLOAD_DRAFT_LIMIT', '最多保留 32 条本机续传记录，请先处理已有任务')
    return this.save({
      schemaVersion: 1,
      createdAt: new Date().toISOString(),
      request: { ...input },
      uploadId: null,
      pendingAction: null,
    })
  }
  bind(id: string, uploadId: string): UploadDraft {
    const row = this.get(id)
    if (!objectId(uploadId) || (row.uploadId !== null && row.uploadId !== uploadId))
      throw new ApiError('UPLOAD_DRAFT_CONFLICT', '上传结果与本机记录不一致')
    return this.save({ ...row, uploadId })
  }
  mark(id: string, kind: UploadAction): UploadDraft {
    if (!actions.includes(kind)) throw invalid()
    return this.save({
      ...this.get(id),
      pendingAction: { actionId: crypto.randomUUID(), kind, requestedAt: new Date().toISOString() },
    })
  }
  clearAction(id: string, expectedActionId?: string): UploadDraft {
    const row = this.get(id)
    if (expectedActionId !== undefined && row.pendingAction?.actionId !== expectedActionId)
      return row
    return this.save({ ...row, pendingAction: null })
  }
  forget(id: string) {
    if (!requestId(id)) throw invalid()
    try {
      this.storage.removeItem(this.prefix + id)
    } catch {
      throw unavailable()
    }
  }
}
