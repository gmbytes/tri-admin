export class ApiError extends Error {
  constructor(code, message, status = 0) {
    super(message)
    this.code = code
    this.status = status
  }
}
const invalid = () => new ApiError('INVALID_RESPONSE', '服务器响应格式无效')
const object = (value) => value !== null && typeof value === 'object' && !Array.isArray(value)
const exact = (value, keys) =>
  object(value) && Object.keys(value).sort().join('|') === [...keys].sort().join('|')
const oid = (value) => typeof value === 'string' && /^[a-f0-9]{24}$/.test(value)
const integer = (value, min = 1, max = Number.MAX_SAFE_INTEGER) =>
  Number.isSafeInteger(value) && value >= min && value <= max
const date = (value) =>
  typeof value === 'string' &&
  /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/.test(value) &&
  Number.isFinite(Date.parse(value)) &&
  new Date(value).toISOString() === value
const length = (value) => [...value].length
export function normalizeEndpoint(raw) {
  let url
  try {
    url = new URL(raw.trim())
  } catch {
    throw new Error('请输入完整服务地址')
  }
  if (
    url.username ||
    url.password ||
    url.search ||
    url.hash ||
    url.port === '0' ||
    !['', '/', '/api/v1'].includes(url.pathname)
  )
    throw new Error('服务地址不能包含账号、查询或额外路径')
  if (
    url.protocol !== 'https:' &&
    !(url.protocol === 'http:' && ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname))
  )
    throw new Error('远程服务需要 HTTPS')
  return url.origin + '/api/v1'
}
export class Client {
  #schema
  #endpoint
  #token = ''
  /** @type {any} */ #user = null
  #epoch = 0
  #controls = new Set()
  constructor(schema, endpoint) {
    if (!/^[a-z][a-z0-9_]{2,47}$/.test(schema.resource) || !Array.isArray(schema.fields))
      throw new Error('资源描述无效')
    this.#schema = schema
    this.#endpoint = normalizeEndpoint(endpoint)
  }
  get user() {
    return this.#user ? { ...this.#user } : null
  }
  clear() {
    this.#epoch++
    this.#token = ''
    this.#user = null
    for (const controller of this.#controls) controller.abort()
    this.#controls.clear()
  }
  setEndpoint(value) {
    const next = normalizeEndpoint(value)
    this.clear()
    this.#endpoint = next
  }
  async #request(method, path, body, anonymous = false) {
    const epoch = this.#epoch
    if (!anonymous && !this.#token) throw new ApiError('AUTH_REQUIRED', '请先登录', 401)
    const controller = new AbortController()
    this.#controls.add(controller)
    const timeout = setTimeout(() => controller.abort(), 10000)
    try {
      const response = await fetch(this.#endpoint + path, {
        method,
        headers: {
          ...(anonymous ? {} : { authorization: 'Bearer ' + this.#token }),
          ...(body === undefined ? {} : { 'content-type': 'application/json' }),
        },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: controller.signal,
      })
      if (epoch !== this.#epoch) throw new ApiError('SESSION_CHANGED', '当前账号或服务已变化')
      if (response.status === 204) return null
      const reader = response.body?.getReader()
      if (!reader) throw invalid()
      const decoder = new TextDecoder('utf-8', { fatal: true })
      let text = '',
        size = 0
      try {
        while (true) {
          const next = await reader.read()
          if (next.done) break
          size += next.value.byteLength
          if (size > 8 * 1024 * 1024) {
            await reader.cancel()
            throw invalid()
          }
          text += decoder.decode(next.value, { stream: true })
        }
        text += decoder.decode()
      } finally {
        reader.releaseLock()
      }
      if (epoch !== this.#epoch) throw new ApiError('SESSION_CHANGED', '当前账号或服务已变化')
      let value
      try {
        value = JSON.parse(text)
      } catch {
        throw invalid()
      }
      if (!response.ok) {
        if (response.status === 401) this.clear()
        const message =
          typeof value?.error?.message === 'string' && value.error.message.length <= 1000
            ? value.error.message
            : '请求失败'
        throw new ApiError(
          response.status === 401 ? 'AUTH_REQUIRED' : 'HTTP_ERROR',
          message,
          response.status,
        )
      }
      return value
    } catch (error) {
      if (epoch !== this.#epoch && !(error instanceof ApiError && error.code === 'AUTH_REQUIRED'))
        throw new ApiError('SESSION_CHANGED', '当前账号或服务已变化')
      if (error instanceof ApiError) throw error
      throw new ApiError('NETWORK_ERROR', '无法连接服务器或响应读取中断')
    } finally {
      clearTimeout(timeout)
      this.#controls.delete(controller)
    }
  }
  async login(username, password) {
    const epoch = this.#epoch,
      result = await this.#request('POST', '/auth/login', { username, password }, true)
    const value = result?.data
    if (
      !exact(result, ['data']) ||
      !exact(value, ['accessToken', 'expiresIn', 'user']) ||
      typeof value.accessToken !== 'string' ||
      !/^[-_A-Za-z0-9]+\.[-_A-Za-z0-9]+\.[-_A-Za-z0-9]+$/.test(value.accessToken) ||
      value.accessToken.length > 8192 ||
      !integer(value.expiresIn, 1, 900) ||
      !exact(value.user, ['id', 'username', 'canWrite', 'dataScope']) ||
      !oid(value.user.id) ||
      typeof value.user.username !== 'string' ||
      length(value.user.username) > 64 ||
      typeof value.user.canWrite !== 'boolean' ||
      !['all', 'self'].includes(value.user.dataScope)
    )
      throw invalid()
    if (epoch !== this.#epoch) throw new ApiError('SESSION_CHANGED', '当前账号或服务已变化')
    this.#token = value.accessToken
    this.#user = { ...value.user }
    return this.user
  }
  async logout() {
    const epoch = this.#epoch
    try {
      if (this.#token) await this.#request('POST', '/auth/logout', {})
    } finally {
      if (epoch === this.#epoch) this.clear()
    }
  }
  #values(values) {
    if (
      !exact(
        values,
        this.#schema.fields.map((field) => field.name),
      )
    )
      throw invalid()
    try {
      if (new TextEncoder().encode(JSON.stringify(values)).byteLength > 64 * 1024) throw invalid()
    } catch {
      throw invalid()
    }
    for (const field of this.#schema.fields) {
      const value = values[field.name]
      if (value === null) {
        if (field.required) throw invalid()
        continue
      }
      let valid = false
      switch (field.kind) {
        case 'text':
          valid =
            typeof value === 'string' &&
            length(value) <= 4096 &&
            (!field.required || !!value.trim())
          break
        case 'integer':
          valid = integer(value, -Number.MAX_SAFE_INTEGER)
          break
        case 'number':
          valid =
            typeof value === 'number' &&
            Number.isFinite(value) &&
            Math.abs(value) <= Number.MAX_SAFE_INTEGER
          break
        case 'boolean':
          valid = typeof value === 'boolean'
          break
        case 'datetime':
          valid = date(value)
          break
        case 'textList':
          valid =
            Array.isArray(value) &&
            value.length <= 100 &&
            value.every((item) => typeof item === 'string' && length(item) <= 256)
          break
      }
      if (!valid) throw invalid()
    }
    return values
  }
  #record(value) {
    if (
      !exact(value, ['id', 'ownerId', 'version', 'createdAt', 'updatedAt', 'values']) ||
      !oid(value.id) ||
      !oid(value.ownerId) ||
      !integer(value.version) ||
      !date(value.createdAt) ||
      !date(value.updatedAt)
    )
      throw invalid()
    this.#values(value.values)
    return value
  }
  async list({ page = 1, pageSize = 20, search = '' } = {}) {
    if (
      !integer(page, 1, 100000) ||
      !integer(pageSize, 1, 100) ||
      typeof search !== 'string' ||
      length(search) > 100
    )
      throw new Error('分页或搜索无效')
    const query = new URLSearchParams({ page: String(page), pageSize: String(pageSize), search })
    const value = await this.#request('GET', '/' + this.#schema.resource + '?' + query)
    if (
      !exact(value, ['data', 'meta']) ||
      !Array.isArray(value.data) ||
      !exact(value.meta, ['page', 'pageSize', 'total']) ||
      value.meta.page !== page ||
      value.meta.pageSize !== pageSize ||
      !integer(value.meta.total, 0) ||
      value.data.length > pageSize ||
      (value.data.length > 0 && value.meta.total < (page - 1) * pageSize + value.data.length)
    )
      throw invalid()
    return { data: value.data.map((row) => this.#record(row)), meta: value.meta }
  }
  async get(id) {
    if (!oid(id)) throw new Error('记录 ID 无效')
    const value = await this.#request('GET', '/' + this.#schema.resource + '/' + id)
    if (!exact(value, ['data']) || value.data?.id !== id) throw invalid()
    return this.#record(value.data)
  }
  async create(values) {
    this.#input(values)
    const value = await this.#request('POST', '/' + this.#schema.resource, { values })
    if (!exact(value, ['data'])) throw invalid()
    return this.#record(value.data)
  }
  async update(id, version, values) {
    if (!oid(id) || !integer(version, 1, Number.MAX_SAFE_INTEGER - 1))
      throw new Error('记录或版本无效')
    this.#input(values)
    const value = await this.#request('PATCH', '/' + this.#schema.resource + '/' + id, {
      version,
      values,
    })
    if (!exact(value, ['data']) || value.data?.id !== id) throw invalid()
    return this.#record(value.data)
  }
  async remove(id, version) {
    if (!oid(id) || !integer(version, 1, Number.MAX_SAFE_INTEGER - 1))
      throw new Error('记录或版本无效')
    await this.#request('DELETE', '/' + this.#schema.resource + '/' + id + '?version=' + version)
  }
  #input(values) {
    try {
      this.#values(values)
    } catch {
      throw new ApiError('INVALID_REQUEST', '请检查必填项、字段类型及 64 KiB 记录上限')
    }
  }
}
