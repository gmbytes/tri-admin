import { normalizeEndpoint } from './endpoint'
import type { Envelope, TokenResponse, UserSummary } from './types'

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
    options: RequestOptions = {},
    access?: string | null,
  ): Promise<Envelope<T>> {
    if (
      !path.startsWith('/') ||
      path.startsWith('//') ||
      path.includes('://') ||
      path.includes('\\')
    )
      throw new ApiError('INVALID_REQUEST', '请求地址无效')
    let response: Response
    try {
      response = await fetch(`${endpoint}${path}`, {
        method: options.method ?? 'GET',
        headers: {
          ...(options.body !== undefined ? { 'content-type': 'application/json' } : {}),
          ...(access ? { authorization: `Bearer ${access}` } : {}),
        },
        body: options.body !== undefined ? JSON.stringify(options.body) : undefined,
        signal: AbortSignal.timeout(30_000),
        credentials: 'omit',
      })
    } catch {
      throw new ApiError('NETWORK_ERROR', '无法连接服务器，请检查地址和网络')
    }
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
      refreshToken = this.refreshToken
    task.promise = Promise.resolve()
      .then(async () => {
        this.check(epoch)
        if (!refreshToken) throw new ApiError('AUTH_REQUIRED', '请先登录')
        const tokens = parseTokens(
          await this.send(endpoint, '/auth/refresh', { method: 'POST', body: { refreshToken } }),
        )
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

  async request<T>(path: string, options: RequestOptions = {}): Promise<Envelope<T>> {
    const epoch = this.epoch,
      endpoint = this.base,
      originalAccess = this.access
    if (!originalAccess) throw new ApiError('AUTH_REQUIRED', '请先登录')
    let result: Envelope<T>
    try {
      result = await this.send<T>(endpoint, path, options, originalAccess)
    } catch (error) {
      this.check(epoch)
      if (!(error instanceof ApiError) || error.status !== 401 || error.code !== 'UNAUTHENTICATED')
        throw error
      if (this.access === originalAccess) await this.refreshOnce(epoch)
      this.check(epoch)
      result = await this.send<T>(endpoint, path, options, this.access)
    }
    this.check(epoch)
    return result
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
