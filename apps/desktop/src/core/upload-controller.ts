import { ApiError } from './client'
import { normalizeEndpoint } from './endpoint'
import { FILE_CHUNK_SIZE, type UploadSource } from './files'
import { objectId, parseUploadRecord, type UploadAction, type UploadRecord } from './file-models'
import { UploadJournal, type UploadDraft, type UploadScope } from './upload-journal'
import type { Envelope } from './types'
export interface UploadTransport {
  request<T>(
    path: string,
    options?: { method?: string; body?: unknown; signal?: AbortSignal },
  ): Promise<Envelope<T>>
  putChunk<T>(
    uploadId: string,
    index: number,
    bytes: Blob,
    sha256: string,
    options?: { signal?: AbortSignal },
  ): Promise<Envelope<T>>
}
interface Options {
  journal?: UploadJournal
  isCurrent: () => boolean
  onUpdate?: (row: UploadRecord) => void
  onProgress?: (requestId: string, bytes: number, total: number) => void
}
export class UploadController {
  private active = new Map<string, AbortController>()
  private disposed = false
  readonly scope: Readonly<UploadScope>
  constructor(
    private api: UploadTransport,
    scope: UploadScope,
    private options: Options,
  ) {
    if (!objectId(scope.actorId)) throw new ApiError('UPLOAD_SCOPE_INVALID', '上传账号无效')
    this.scope = Object.freeze({
      endpoint: normalizeEndpoint(scope.endpoint),
      actorId: scope.actorId,
    })
    if (
      options.journal &&
      (options.journal.scope.endpoint !== this.scope.endpoint ||
        options.journal.scope.actorId !== this.scope.actorId)
    )
      throw new ApiError('UPLOAD_SCOPE_INVALID', '续传记录不属于当前服务器或账号')
  }
  private check(signal?: AbortSignal) {
    if (this.disposed || !this.options.isCurrent())
      throw new ApiError('SESSION_CHANGED', '当前会话已切换')
    if (signal?.aborted) throw new ApiError('REQUEST_CANCELLED', '上传操作已暂停')
  }
  private journal() {
    if (!this.options.journal)
      throw new ApiError('UPLOAD_JOURNAL_UNAVAILABLE', '无法保存本机续传记录，暂不能新建或续传')
    return this.options.journal
  }
  private local<T>(signal: AbortSignal, start: () => Promise<T>): Promise<T> {
    this.check(signal)
    return new Promise<T>((resolve, reject) => {
      const abort = () => reject(new ApiError('REQUEST_CANCELLED', '上传操作已暂停'))
      signal.addEventListener('abort', abort, { once: true })
      Promise.resolve()
        .then(() => {
          this.check(signal)
          return start()
        })
        .then(resolve, reject)
        .finally(() => signal.removeEventListener('abort', abort))
    })
  }
  private async call<T>(
    path: string,
    signal: AbortSignal,
    options: { method?: string; body?: unknown } = {},
  ): Promise<T> {
    this.check(signal)
    const result = await this.api.request<T>(path, { ...options, signal })
    this.check(signal)
    return result.data
  }
  private emit(row: UploadRecord, signal: AbortSignal) {
    this.check(signal)
    this.options.onUpdate?.(row)
  }
  private parse(value: unknown, expected?: UploadDraft) {
    const row = parseUploadRecord(value, this.scope.actorId, expected?.request, true)
    if (expected?.uploadId && expected.uploadId !== row.id)
      throw new ApiError('UPLOAD_RESPONSE_INVALID', '上传结果标识不一致')
    return row
  }
  private async ensure(draft: UploadDraft, signal: AbortSignal): Promise<UploadRecord> {
    if (draft.uploadId) return this.read(draft.uploadId, signal, draft)
    const value = await this.call<unknown>('/uploads', signal, {
        method: 'POST',
        body: draft.request,
      }),
      row = this.parse(value, draft)
    try {
      this.journal().bind(draft.request.requestId, row.id)
    } catch (error) {
      this.emit(row, signal)
      throw error
    }
    this.emit(row, signal)
    return row
  }
  private async read(id: string, signal: AbortSignal, draft?: UploadDraft): Promise<UploadRecord> {
    if (!objectId(id)) throw new ApiError('UPLOAD_RESPONSE_INVALID', '上传标识无效')
    const row = this.parse(await this.call<unknown>(`/uploads/${id}`, signal), draft)
    if (row.id !== id) throw new ApiError('UPLOAD_RESPONSE_INVALID', '上传结果标识不一致')
    this.emit(row, signal)
    return row
  }
  private async operation<T>(
    key: string,
    run: (signal: AbortSignal) => Promise<T>,
    replace = false,
  ): Promise<T> {
    this.check()
    if (this.active.has(key) && !replace)
      throw new ApiError('UPLOAD_BUSY', '此上传正在处理，请稍候')
    if (replace) this.stop(key)
    const controller = new AbortController()
    this.active.set(key, controller)
    try {
      return await run(controller.signal)
    } finally {
      if (this.active.get(key) === controller) this.active.delete(key)
    }
  }
  isActive(id: string) {
    return this.active.has(id)
  }
  stop(id: string) {
    this.active.get(id)?.abort()
  }
  dispose() {
    this.disposed = true
    for (const controller of this.active.values()) controller.abort()
  }
  async transfer(id: string, source: UploadSource): Promise<UploadRecord> {
    try {
      return await this.operation(id, async (signal) => {
        const draft = this.journal().get(id)
        if (source.size !== draft.request.size || source.sha256 !== draft.request.sha256)
          throw new ApiError('UPLOAD_SOURCE_MISMATCH', '所选文件与原上传内容不一致，请重新选择')
        let row = await this.ensure(draft, signal)
        if (!['receiving', 'failed'].includes(row.state)) return row
        if (row.lastError === 'CHECKSUM_MISMATCH')
          throw new ApiError(
            'UPLOAD_RESTART_REQUIRED',
            '整份文件校验失败，请取消此任务后重新新建上传',
          )
        const accepted = new Set(row.receivedIndices!)
        let received = row.receivedBytes
        this.options.onProgress?.(id, received, row.size)
        for (let index = 0; index < row.totalChunks; index++) {
          this.check(signal)
          if (accepted.has(index)) continue
          const part = await this.local(signal, () => source.readChunk(index, signal))
          this.check(signal)
          const size = Math.min(FILE_CHUNK_SIZE, row.size - index * FILE_CHUNK_SIZE)
          if (part.bytes.size !== size || !/^[a-f0-9]{64}$/.test(part.sha256))
            throw new ApiError('UPLOAD_SOURCE_MISMATCH', '本地文件分片与原上传不一致')
          try {
            this.check(signal)
            const reply = (
              await this.api.putChunk<{
                uploadId: string
                index: number
                size: number
                sha256: string
              }>(row.id, index, part.bytes, part.sha256, { signal })
            ).data
            this.check(signal)
            if (
              reply.uploadId !== row.id ||
              reply.index !== index ||
              reply.size !== size ||
              reply.sha256 !== part.sha256
            )
              throw new ApiError('UPLOAD_RESPONSE_INVALID', '分片确认信息无效，请刷新核对')
          } catch (error) {
            this.check(signal)
            row = await this.read(row.id, signal, this.journal().get(id))
            if (!row.receivedIndices!.includes(index) || row.state !== 'receiving') throw error
          }
          accepted.add(index)
          received = [...accepted].reduce(
            (sum, index) => sum + Math.min(FILE_CHUNK_SIZE, row.size - index * FILE_CHUNK_SIZE),
            0,
          )
          this.check(signal)
          this.options.onProgress?.(id, received, row.size)
        }
        row = await this.read(row.id, signal, this.journal().get(id))
        if (!['receiving', 'failed'].includes(row.state)) return row
        return this.perform(row, 'complete', signal, this.journal().get(id))
      })
    } finally {
      await source.close().catch(() => undefined)
    }
  }
  async query(id: string): Promise<UploadRecord> {
    return this.operation(id, async (signal) => {
      const draft = this.journal().get(id),
        row = await this.ensure(draft, signal)
      const action = draft.pendingAction?.kind
      if (
        (action === 'complete' && row.state !== 'receiving') ||
        (action === 'cancel' &&
          ['cancelled', 'cancelRequested', 'completed'].includes(row.state)) ||
        (action === 'retry' && ['completed', 'cancelled'].includes(row.state)) ||
        action === 'recover' ||
        (action === 'cleanup' && !row.cleanupPending)
      )
        this.journal().clearAction(id, draft.pendingAction?.actionId)
      return row
    })
  }
  async queryKnown(id: string): Promise<UploadRecord> {
    return this.operation(id, (signal) => this.read(id, signal))
  }
  async action(id: string, kind: UploadAction): Promise<UploadRecord> {
    return this.operation(
      id,
      async (signal) => {
        const draft = this.journal().get(id),
          row = await this.ensure(draft, signal)
        return this.perform(row, kind, signal, this.journal().get(id))
      },
      kind === 'cancel',
    )
  }
  async actionKnown(id: string, kind: UploadAction): Promise<UploadRecord> {
    return this.operation(
      id,
      async (signal) => this.perform(await this.read(id, signal), kind, signal),
      kind === 'cancel',
    )
  }
  private async perform(
    row: UploadRecord,
    kind: UploadAction,
    signal: AbortSignal,
    draft?: UploadDraft,
  ): Promise<UploadRecord> {
    if (kind === 'cancel' && row.state === 'completed')
      throw new ApiError('UPLOAD_ALREADY_COMPLETED', '文件已发布，请在文件列表中删除')
    const marker = draft
      ? this.journal().mark(draft.request.requestId, kind).pendingAction!.actionId
      : undefined
    const result = await this.call<unknown>(`/uploads/${row.id}/${kind}`, signal, {
      method: 'POST',
      body: {},
    })
    let latest: UploadRecord
    if (kind === 'cleanup' || kind === 'recover') {
      const value = result as Record<string, unknown> | null
      if (
        !value ||
        typeof value !== 'object' ||
        (kind === 'cleanup' &&
          (value.uploadId !== row.id || typeof value.complete !== 'boolean')) ||
        (kind === 'recover' &&
          !['recovered', 'retired', 'unknown'].every(
            (key) => Number.isSafeInteger(value[key]) && Number(value[key]) >= 0,
          ))
      )
        throw new ApiError('UPLOAD_RESPONSE_INVALID', '上传维护结果无效，请刷新核对')
      latest = await this.read(row.id, signal, draft)
    } else {
      latest = this.parse(result, draft)
      if (latest.id !== row.id || latest.requestId !== row.requestId)
        throw new ApiError('UPLOAD_RESPONSE_INVALID', '上传操作结果不一致')
      this.emit(latest, signal)
    }
    if (draft) this.journal().clearAction(draft.request.requestId, marker)
    return latest
  }
}
