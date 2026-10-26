import { normalizeEndpoint } from './endpoint'
export const openScopes = [
  'open.profile.read',
  'open.profile.write',
  'open.users.read',
  'open.users.write',
] as const
export type OpenScope = (typeof openScopes)[number]
export interface Policy {
  allowedIps: string[]
  minuteLimit: number
  dailyQuota: number
}
export interface ApiToken extends Policy {
  id: string
  requestId: string
  name: string
  scopes: string[]
  status: 'active' | 'expired' | 'revoked'
  createdAt: string
  expiresAt: string
  revokedAt: string | null
  lastUsedAt: string | null
}
export interface Application extends Policy {
  id: string
  requestId: string
  name: string
  code: string
  enabled: boolean
  allowedScopes: OpenScope[]
  credentialVersion: number
  version: number
  createdAt: string
  updatedAt: string
  deletedAt: string | null
}
export interface ApplicationUser {
  id: string
  applicationId: string
  username: string
  displayName: string
  scopes: OpenScope[]
  status: 'active' | 'disabled'
  version: number
  createdAt: string
  updatedAt: string
}
export interface RotationReceipt {
  applicationId: string
  requestId: string
  kind: 'rotateSecret'
  version: number
  credentialVersion: number
  createdAt: string
}
export interface CallRecord {
  id: string
  tokenId?: string
  applicationId?: string
  principalKind?: 'application' | 'user'
  userId?: string | null
  sessionId?: string
  requestId: string
  operationId: string | null
  method: string
  route: string
  ip: string | null
  state: 'started' | 'finished'
  status: number | null
  errorCode: string | null
  durationMs: number | null
  createdAt: string
  finishedAt: string | null
}
export interface Page<T> {
  data: T[]
  meta: { page: number; pageSize: number; total: number }
}
export function invalid(): Error {
  return new Error('开放访问响应或恢复记录无效')
}
function object(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === 'object' && !Array.isArray(value)
}
function exact(value: Record<string, unknown>, keys: string[]) {
  return Object.keys(value).sort().join('|') === [...keys].sort().join('|')
}
export function objectId(value: unknown): value is string {
  return typeof value === 'string' && /^[a-f0-9]{24}$/.test(value)
}
export function requestId(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(value)
  )
}
function text(value: unknown, max = 128): value is string {
  return typeof value === 'string' && value.length <= max && !/[\u0000-\u001f\u007f]/.test(value)
}
function date(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/.test(value) &&
    Number.isFinite(Date.parse(value)) &&
    new Date(value).toISOString() === value
  )
}
function nullableDate(value: unknown) {
  return value === null || date(value)
}
function integer(value: unknown, min = 1, max = Number.MAX_SAFE_INTEGER): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= min && value <= max
}
function strings(value: unknown, max: number): value is string[] {
  return (
    Array.isArray(value) &&
    value.length <= max &&
    value.every((v) => text(v)) &&
    new Set(value).size === value.length
  )
}
function oneOf(value: unknown, allowed: readonly string[]): value is string {
  return typeof value === 'string' && allowed.includes(value)
}
function policy(value: Record<string, unknown>) {
  return (
    strings(value.allowedIps, 32) &&
    integer(value.minuteLimit, 1, 10000) &&
    integer(value.dailyQuota, 1, 10000000)
  )
}
function scopes(value: unknown, profile = false): value is OpenScope[] {
  return (
    strings(value, 4) &&
    value.length > 0 &&
    value.every(
      (v) =>
        (openScopes as readonly string[]).includes(v) &&
        (!profile || v.startsWith('open.profile.')),
    )
  )
}
export function parseToken(value: unknown): ApiToken {
  if (
    !object(value) ||
    !exact(value, [
      'id',
      'requestId',
      'name',
      'scopes',
      'allowedIps',
      'minuteLimit',
      'dailyQuota',
      'status',
      'createdAt',
      'expiresAt',
      'revokedAt',
      'lastUsedAt',
    ]) ||
    !objectId(value.id) ||
    !requestId(value.requestId) ||
    !text(value.name, 256) ||
    !strings(value.scopes, 128) ||
    !value.scopes.length ||
    value.scopes.includes('*') ||
    !policy(value) ||
    !oneOf(value.status, ['active', 'expired', 'revoked']) ||
    !date(value.createdAt) ||
    !date(value.expiresAt) ||
    !nullableDate(value.revokedAt) ||
    !nullableDate(value.lastUsedAt)
  )
    throw invalid()
  return value as unknown as ApiToken
}
export function parseApplication(value: unknown): Application {
  if (
    !object(value) ||
    !exact(value, [
      'id',
      'requestId',
      'name',
      'code',
      'enabled',
      'allowedScopes',
      'allowedIps',
      'minuteLimit',
      'dailyQuota',
      'credentialVersion',
      'version',
      'createdAt',
      'updatedAt',
      'deletedAt',
    ]) ||
    !objectId(value.id) ||
    !requestId(value.requestId) ||
    !text(value.name, 320) ||
    typeof value.code !== 'string' ||
    !/^[a-z][a-z0-9._-]{2,63}$/.test(value.code) ||
    typeof value.enabled !== 'boolean' ||
    !scopes(value.allowedScopes) ||
    !policy(value) ||
    !integer(value.version) ||
    !integer(value.credentialVersion) ||
    !date(value.createdAt) ||
    !date(value.updatedAt) ||
    !nullableDate(value.deletedAt)
  )
    throw invalid()
  return value as unknown as Application
}
export function parseApplicationUser(value: unknown): ApplicationUser {
  if (
    !object(value) ||
    !exact(value, [
      'id',
      'applicationId',
      'username',
      'displayName',
      'scopes',
      'status',
      'version',
      'createdAt',
      'updatedAt',
    ]) ||
    !objectId(value.id) ||
    !objectId(value.applicationId) ||
    !text(value.username, 256) ||
    !text(value.displayName, 320) ||
    !scopes(value.scopes, true) ||
    !oneOf(value.status, ['active', 'disabled']) ||
    !integer(value.version) ||
    !date(value.createdAt) ||
    !date(value.updatedAt)
  )
    throw invalid()
  return value as unknown as ApplicationUser
}
export function parseReceipt(value: unknown): RotationReceipt {
  if (
    !object(value) ||
    !exact(value, [
      'applicationId',
      'requestId',
      'kind',
      'version',
      'credentialVersion',
      'createdAt',
    ]) ||
    !objectId(value.applicationId) ||
    !requestId(value.requestId) ||
    value.kind !== 'rotateSecret' ||
    !integer(value.version) ||
    !integer(value.credentialVersion) ||
    !date(value.createdAt)
  )
    throw invalid()
  return value as unknown as RotationReceipt
}
export function parsePage<T>(value: unknown, parse: (value: unknown) => T): Page<T> {
  if (
    !object(value) ||
    !exact(value, ['data', 'meta']) ||
    !Array.isArray(value.data) ||
    !object(value.meta) ||
    !exact(value.meta, ['page', 'pageSize', 'total']) ||
    !integer(value.meta.page) ||
    !integer(value.meta.pageSize, 1, 100) ||
    !integer(value.meta.total, 0) ||
    value.data.length > value.meta.pageSize ||
    (value.data.length > 0 &&
      value.meta.total < (value.meta.page - 1) * value.meta.pageSize + value.data.length)
  )
    throw invalid()
  return { data: value.data.map(parse), meta: value.meta as Page<T>['meta'] }
}
function secret(value: unknown, kind: 'token' | 'application', id: string): string | null {
  if (value === null) return null
  const prefix = kind === 'token' ? 'tri_at1' : 'tri_app1'
  if (
    typeof value !== 'string' ||
    !new RegExp(`^${prefix}\\.${id}\\.[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$`).test(value)
  )
    throw invalid()
  return value
}
export function parseIssue(
  value: unknown,
  kind: 'token',
): { entity: ApiToken; secret: string | null }
export function parseIssue(
  value: unknown,
  kind: 'application',
): { entity: Application; secret: string | null }
export function parseIssue(
  value: unknown,
  kind: 'token' | 'application',
): { entity: ApiToken | Application; secret: string | null } {
  const field = kind === 'token' ? 'token' : 'application'
  if (!object(value) || !exact(value, [field, 'secret'])) throw invalid()
  const entity = kind === 'token' ? parseToken(value[field]) : parseApplication(value[field])
  return { entity, secret: secret(value.secret, kind, entity.id) }
}
export function parseRotation(value: unknown) {
  if (!object(value) || !exact(value, ['application', 'secret', 'receipt'])) throw invalid()
  const entity = parseApplication(value.application),
    receipt = parseReceipt(value.receipt)
  if (
    entity.id !== receipt.applicationId ||
    entity.version < receipt.version ||
    entity.credentialVersion < receipt.credentialVersion
  )
    throw invalid()
  return { entity, receipt, secret: secret(value.secret, 'application', entity.id) }
}
export function parseCall(value: unknown, kind: 'token' | 'application'): CallRecord {
  const shared = [
      'id',
      'requestId',
      'operationId',
      'method',
      'route',
      'ip',
      'state',
      'status',
      'errorCode',
      'durationMs',
      'createdAt',
      'finishedAt',
    ],
    extra =
      kind === 'token' ? ['tokenId'] : ['applicationId', 'principalKind', 'userId', 'sessionId']
  if (
    !object(value) ||
    !exact(value, [...shared, ...extra]) ||
    !objectId(value.id) ||
    !requestId(value.requestId) ||
    !(value.operationId === null || text(value.operationId)) ||
    !text(value.method, 16) ||
    !text(value.route, 256) ||
    !(value.ip === null || text(value.ip, 64)) ||
    !oneOf(value.state, ['started', 'finished']) ||
    !(value.status === null || integer(value.status, 100, 599)) ||
    !(value.errorCode === null || text(value.errorCode)) ||
    !(value.durationMs === null || integer(value.durationMs, 0)) ||
    !date(value.createdAt) ||
    !nullableDate(value.finishedAt)
  )
    throw invalid()
  if (
    kind === 'token'
      ? !objectId(value.tokenId)
      : !objectId(value.applicationId) ||
        !oneOf(value.principalKind, ['application', 'user']) ||
        !(value.userId === null || objectId(value.userId)) ||
        !objectId(value.sessionId)
  )
    throw invalid()
  return value as unknown as CallRecord
}
export interface CredentialOperation {
  requestId: string
  kind: 'token.create' | 'app.create' | 'app.rotate'
  applicationId: string | null
  version: number | null
  createdAt: string
}
function operation(value: unknown): CredentialOperation {
  if (
    !object(value) ||
    !exact(value, ['requestId', 'kind', 'applicationId', 'version', 'createdAt']) ||
    !requestId(value.requestId) ||
    !date(value.createdAt) ||
    !oneOf(value.kind, ['token.create', 'app.create', 'app.rotate'])
  )
    throw invalid()
  if (
    value.kind === 'app.rotate'
      ? !objectId(value.applicationId) || !integer(value.version)
      : value.applicationId !== null || value.version !== null
  )
    throw invalid()
  return { ...value } as unknown as CredentialOperation
}
export class CredentialRecoveryStore {
  private key: string
  constructor(
    private storage: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>,
    endpoint: string,
    actor: string,
  ) {
    if (!objectId(actor)) throw invalid()
    this.key = `tri-admin.credentials.v1:${encodeURIComponent(normalizeEndpoint(endpoint))}:${actor}`
  }
  read(): CredentialOperation[] {
    const raw = this.storage.getItem(this.key)
    if (raw === null) return []
    if (raw.length > 16384) throw invalid()
    let value: unknown
    try {
      value = JSON.parse(raw)
    } catch {
      throw invalid()
    }
    if (
      !object(value) ||
      !exact(value, ['schemaVersion', 'operations']) ||
      value.schemaVersion !== 1 ||
      !Array.isArray(value.operations) ||
      value.operations.length > 16
    )
      throw invalid()
    const rows = value.operations.map(operation)
    if (new Set(rows.map((row) => row.requestId)).size !== rows.length) throw invalid()
    return rows
  }
  put(value: CredentialOperation) {
    const row = operation(value),
      rows = this.read(),
      old = rows.find((v) => v.requestId === row.requestId)
    if (
      old &&
      (old.kind !== row.kind ||
        old.applicationId !== row.applicationId ||
        old.version !== row.version)
    )
      throw invalid()
    const next = rows.filter((v) => v.requestId !== row.requestId)
    next.push(row)
    if (next.length > 16) throw new Error('最多保留 16 个待确认凭据操作')
    this.storage.setItem(this.key, JSON.stringify({ schemaVersion: 1, operations: next }))
  }
  remove(id: string) {
    const rows = this.read().filter((row) => row.requestId !== id)
    if (rows.length)
      this.storage.setItem(this.key, JSON.stringify({ schemaVersion: 1, operations: rows }))
    else this.storage.removeItem(this.key)
  }
}
export function policyFromForm(ips: string, minuteLimit: number, dailyQuota: number): Policy {
  const allowedIps = ips
    .split(/[\n,]/)
    .map((v) => v.trim())
    .filter(Boolean)
  if (
    allowedIps.length > 32 ||
    !integer(minuteLimit, 1, 10000) ||
    !integer(dailyQuota, 1, 10000000)
  )
    throw new Error('请检查来源地址和调用额度')
  return { allowedIps, minuteLimit, dailyQuota }
}
