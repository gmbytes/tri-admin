import { normalizeEndpoint } from './endpoint'
import type { Envelope, TokenResponse, UserSummary } from './types'
import {
  FileTransferError,
  readFileContent,
  validateFileSpec,
  type FileContentSpec,
  type FileSink,
} from './files'

export interface SessionVault {
  load(endpoint: string): Promise<string | null>
  save(endpoint: string, token: string): Promise<void>
  clear(endpoint: string): Promise<void>
}
export class ApiError extends Error {
  constructor(
    public code: string,
    message: string,
    public status = 0,
    public requestId?: string,
  ) {
    super(message)
    this.name = 'ApiError'
  }
}
interface Options {
  endpoint: string
  vault: SessionVault
  onChange?: (user: UserSummary | null) => void
}
interface RequestOptions {
  method?: string
  body?: unknown
  signal?: AbortSignal
}
interface TransportOptions extends RequestOptions {
  chunk?: { body: Blob; sha256: string }
  timeoutMs?: number
}

function parseTokens(value: unknown): TokenResponse {
  const data = (value as Envelope<TokenResponse>)?.data
  const user = data?.user
  if (
    !data ||
    typeof data.accessToken !== 'string' ||
    !data.accessToken ||
    typeof data.refreshToken !== 'string' ||
    !/^[a-f0-9]{64}$/.test(data.refreshToken) ||
    !Number.isFinite(data.expiresIn) ||
    data.expiresIn <= 0 ||
    !user ||
    typeof user.id !== 'string' ||
    typeof user.username !== 'string' ||
    typeof user.displayName !== 'string' ||
    !Array.isArray(user.roleIds) ||
    !user.roleIds.every((value) => typeof value === 'string') ||
    !Array.isArray(user.permissions) ||
    !user.permissions.every((value) => typeof value === 'string') ||
    !['all', 'self', 'roles'].includes(user.dataScope) ||
    !Array.isArray(user.dataScopeRoleIds) ||
    !user.dataScopeRoleIds.every(
      (value) => typeof value === 'string' && /^[a-f0-9]{24}$/.test(value),
    ) ||
    (user.dataScope === 'roles'
      ? user.dataScopeRoleIds.length === 0
      : user.dataScopeRoleIds.length !== 0)
  )
    throw new ApiError('INVALID_RESPONSE', '服务器返回的数据格式无效')
  return {
    accessToken: data.accessToken,
    refreshToken: data.refreshToken,
    expiresIn: data.expiresIn,
    user: {
      id: user.id,
      username: user.username,
      displayName: user.displayName,
      roleIds: [...user.roleIds],
      permissions: [...user.permissions],
      dataScope: user.dataScope,
      dataScopeRoleIds: [...user.dataScopeRoleIds],
    },
  }
}

export class ApiClient {
  private base: string
  private access: string | null = null
  private refreshToken: string | null = null
  private currentUser: UserSummary | null = null
  private epoch = 0
  private sessionAbort = new AbortController()
  private remember = false
  private refreshTask: { epoch: number; promise: Promise<void> } | null = null
  private vaultQueue: Promise<unknown> = Promise.resolve()
  constructor(private options: Options) {
    this.base = normalizeEndpoint(options.endpoint)
  }
  get endpoint() {
    return this.base
  }
  get user() {
    return this.currentUser
  }

  private check(epoch: number) {
    if (epoch !== this.epoch) throw new ApiError('SESSION_CHANGED', '当前会话已切换')
  }
  private publish(user: UserSummary | null) {
    this.currentUser = user
    this.options.onChange?.(user)
  }
  private invalidate() {
    this.epoch++
    this.sessionAbort.abort()
    this.sessionAbort = new AbortController()
    this.access = null
    this.refreshToken = null
    this.publish(null)
  }
  private queue<T>(operation: () => Promise<T>): Promise<T> {
    const next = this.vaultQueue.then(operation, operation)
    this.vaultQueue = next.catch(() => undefined)
    return next
  }
  private async accept(tokens: TokenResponse, epoch: number, endpoint: string) {
    this.check(epoch)
    try {
      await this.queue(async () => {
        this.check(epoch)
        if (this.remember) await this.options.vault.save(endpoint, tokens.refreshToken)
        else await this.options.vault.clear(endpoint)
      })
    } catch (error) {
      if (error instanceof ApiError && error.code === 'SESSION_CHANGED') throw error
      throw new ApiError('SESSION_STORAGE_ERROR', '本机会话存储暂不可用，请取消“保持登录”后重试')
    }
    this.check(epoch)
    this.access = tokens.accessToken
    this.refreshToken = tokens.refreshToken
    this.publish(tokens.user)
  }
  private async rollback(tokens: TokenResponse, epoch: number, endpoint: string) {
    let clearing: Promise<unknown> = Promise.resolve()
    if (epoch === this.epoch) {
      this.invalidate()
      const clearedEpoch = this.epoch
      clearing = this.queue(async () => {
        if (clearedEpoch === this.epoch) await this.options.vault.clear(endpoint)
      }).catch(() => undefined)
    }
    await this.send(endpoint, '/auth/logout', {
      method: 'POST',
      body: { refreshToken: tokens.refreshToken },
    }).catch(() => undefined)
    await clearing
  }

  private async send<T>(
    endpoint: string,
    path: string,
    options: TransportOptions = {},
    access?: string | null,
    decode?: (response: Response, signal: AbortSignal) => Promise<Envelope<T>>,
  ): Promise<Envelope<T>> {
    if (
      !path.startsWith('/') ||
      path.startsWith('//') ||
      path.includes('://') ||
      path.includes('\\')
    )
      throw new ApiError('INVALID_REQUEST', '请求地址无效')
    let response: Response
    const signal = AbortSignal.any([
      AbortSignal.timeout(options.timeoutMs ?? 30_000),
      ...(options.signal ? [options.signal] : []),
    ])
    try {
      response = await fetch(`${endpoint}${path}`, {
        method: options.method ?? 'GET',
        headers: {
          ...(options.chunk
            ? { 'content-type': 'application/octet-stream', 'x-chunk-sha256': options.chunk.sha256 }
            : options.body !== undefined
              ? { 'content-type': 'application/json' }
              : {}),
          ...(access ? { authorization: `Bearer ${access}` } : {}),
        },
        body:
          options.chunk?.body ??
          (options.body !== undefined ? JSON.stringify(options.body) : undefined),
        signal,
        credentials: 'omit',
        redirect: 'error',
      })
    } catch {
      if (options.signal?.aborted) throw new ApiError('REQUEST_CANCELLED', '操作已取消')
      throw new ApiError('NETWORK_ERROR', '无法连接服务器，请检查地址和网络')
    }
    if (response.ok && decode) return decode(response, signal)
    if (response.status === 204) return { data: undefined as T }
    let value: unknown
    try {
      value = await response.json()
    } catch {
      throw new ApiError('INVALID_RESPONSE', '服务器返回的数据格式无效', response.status)
    }
    if (!response.ok) {
      const error = (value as { error?: { code?: unknown; message?: unknown } })?.error
      throw new ApiError(
        typeof error?.code === 'string' ? error.code : 'HTTP_ERROR',
        typeof error?.message === 'string' ? error.message.slice(0, 500) : '请求失败，请稍后重试',
        response.status,
        response.headers.get('x-request-id') ?? undefined,
      )
    }
    if (!value || typeof value !== 'object' || !Object.hasOwn(value, 'data'))
      throw new ApiError('INVALID_RESPONSE', '服务器返回的数据格式无效', response.status)
    return value as Envelope<T>
  }

  async login(username: string, password: string, remember: boolean): Promise<UserSummary> {
    if (this.refreshToken) await this.logout()
    this.invalidate()
    this.remember = remember
    const epoch = this.epoch,
      endpoint = this.base
    const tokens = parseTokens(
      await this.send(endpoint, '/auth/login', { method: 'POST', body: { username, password } }),
    )
    try {
      await this.accept(tokens, epoch, endpoint)
    } catch (error) {
      await this.rollback(tokens, epoch, endpoint)
      if (error instanceof ApiError) throw error
      throw new ApiError('SESSION_STORAGE_ERROR', '无法保存本机会话，请取消“保持登录”后重试')
    }
    return tokens.user
  }

  async restore(): Promise<UserSummary | null> {
    const epoch = this.epoch,
      endpoint = this.base
    const token = await this.queue(() => this.options.vault.load(endpoint))
    this.check(epoch)
    if (!token) return null
    this.remember = true
    this.refreshToken = token
    await this.refreshOnce(epoch)
    return this.currentUser
  }

  private refreshOnce(epoch: number): Promise<void> {
    if (this.refreshTask?.epoch === epoch) return this.refreshTask.promise
    const task = { epoch, promise: Promise.resolve() }
    const endpoint = this.base,
      refreshToken = this.refreshToken,
      expectedActor = this.currentUser?.id
    task.promise = Promise.resolve()
      .then(async () => {
        this.check(epoch)
        if (!refreshToken) throw new ApiError('AUTH_REQUIRED', '请先登录')
        const tokens = parseTokens(
          await this.send(endpoint, '/auth/refresh', { method: 'POST', body: { refreshToken } }),
        )
        if (expectedActor && tokens.user.id !== expectedActor) {
          await this.rollback(tokens, epoch, endpoint)
          throw new ApiError('INVALID_RESPONSE', '服务器返回的会话身份不一致，请重新登录')
        }
        try {
          await this.accept(tokens, epoch, endpoint)
        } catch (error) {
          await this.rollback(tokens, epoch, endpoint)
          throw error
        }
      })
      .catch(async (error) => {
        if (epoch === this.epoch && error instanceof ApiError && error.status === 401) {
          this.invalidate()
          await this.queue(() => this.options.vault.clear(endpoint))
        }
        throw error
      })
      .finally(() => {
        if (this.refreshTask === task) this.refreshTask = null
      })
    this.refreshTask = task
    return task.promise
  }

  private async authorized<T>(
    path: string,
    options: TransportOptions,
    decode?: (response: Response, signal: AbortSignal) => Promise<Envelope<T>>,
  ): Promise<Envelope<T>> {
    const epoch = this.epoch,
      endpoint = this.base,
      originalAccess = this.access
    if (!originalAccess) throw new ApiError('AUTH_REQUIRED', '请先登录')
    const scoped = {
      ...options,
      signal: AbortSignal.any([
        this.sessionAbort.signal,
        ...(options.signal ? [options.signal] : []),
      ]),
    }
    let result: Envelope<T>
    try {
      result = await this.send<T>(endpoint, path, scoped, originalAccess, decode)
    } catch (error) {
      this.check(epoch)
      if (!(error instanceof ApiError) || error.status !== 401 || error.code !== 'UNAUTHENTICATED')
        throw error
      if (this.access === originalAccess) await this.refreshOnce(epoch)
      this.check(epoch)
      result = await this.send<T>(endpoint, path, scoped, this.access, decode)
    }
    this.check(epoch)
    return result
  }

  async request<T>(path: string, options: RequestOptions = {}): Promise<Envelope<T>> {
    return this.authorized<T>(path, options)
  }

  async putChunk<T = unknown>(
    uploadId: string,
    index: number,
    bytes: Blob,
    sha256: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<Envelope<T>> {
    if (
      !/^[a-f0-9]{24}$/.test(uploadId) ||
      !Number.isInteger(index) ||
      index < 0 ||
      index >= 1280 ||
      !(bytes instanceof Blob) ||
      bytes.size === 0 ||
      bytes.size > 4 * 1024 * 1024 ||
      !/^[a-f0-9]{64}$/.test(sha256)
    )
      throw new ApiError('INVALID_FILE_CHUNK', '分片标识、索引、长度或摘要无效')
    return this.authorized<T>(`/uploads/${uploadId}/chunks/${index}`, {
      method: 'PUT',
      chunk: { body: bytes, sha256 },
      signal: options.signal,
    })
  }

  async downloadFile<T>(
    fileId: string,
    expected: FileContentSpec,
    sink: FileSink<T>,
    options: { signal?: AbortSignal; onProgress?: (bytes: number) => void } = {},
  ): Promise<T> {
    return this.downloadContent(fileId, expected, sink, options)
  }

  private async downloadContent<T>(
    fileId: string,
    expected: FileContentSpec,
    sink: FileSink<T>,
    options: { signal?: AbortSignal; onProgress?: (bytes: number) => void },
  ): Promise<T> {
    let cleanup: Promise<void> | undefined
    const abort = () =>
      (cleanup ??= Promise.resolve()
        .then(() => sink.abort())
        .catch(() => undefined))
    const target: FileSink<T> = {
      write: (bytes) => sink.write(bytes),
      complete: (value) => sink.complete(value),
      abort,
    }
    try {
      if (!/^[a-f0-9]{24}$/.test(fileId)) throw new ApiError('INVALID_FILE', '文件标识无效')
      validateFileSpec(expected)
      const spec = Object.freeze({ size: expected.size, sha256: expected.sha256 })
      const result = await this.authorized<T>(
        `/files/${fileId}/content`,
        { signal: options.signal, timeoutMs: 30 * 60 * 1000 },
        async (response, signal) => ({
          data: await readFileContent(response, spec, target, {
            signal,
            onProgress: options.onProgress,
            mediaType: 'application/octet-stream',
          }),
        }),
      )
      return result.data
    } catch (error) {
      await abort()
      if (error instanceof ApiError) throw error
      if (error instanceof FileTransferError) throw new ApiError(error.code, error.message)
      throw new ApiError('FILE_TRANSFER_FAILED', '文件下载失败，请重试')
    }
  }
  async clearSession(): Promise<void> {
    const endpoint = this.base
    this.invalidate()
    await this.queue(() => this.options.vault.clear(endpoint))
  }
  async logout(): Promise<boolean> {
    const endpoint = this.base,
      refreshToken = this.refreshToken
    this.invalidate()
    const revoked = refreshToken
      ? this.send(endpoint, '/auth/logout', { method: 'POST', body: { refreshToken } }).then(
          () => true,
          () => false,
        )
      : Promise.resolve(true)
    await this.queue(() => this.options.vault.clear(endpoint))
    return revoked
  }
  async setEndpoint(raw: string): Promise<void> {
    const endpoint = normalizeEndpoint(raw)
    await this.clearSession()
    this.base = endpoint
  }
}
