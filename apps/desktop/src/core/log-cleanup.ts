import type { LogCleanupRequest, LogCleanupResult } from './types'
export interface JournalStorage {
  readonly length: number
  key(index: number): string | null
  getItem(key: string): string | null
  setItem(key: string, value: string): void
  removeItem(key: string): void
}
export interface CleanupScope {
  endpoint: string
  actorId: string
}
function prefix(scope: CleanupScope) {
  return `tri-admin.log-cleanup:${encodeURIComponent(scope.endpoint)}:${encodeURIComponent(scope.actorId)}:`
}
function valid(value: unknown): value is LogCleanupRequest {
  if (!value || typeof value !== 'object') return false
  const entry = value as Record<string, unknown>
  return (
    Object.keys(entry).sort().join(',') === 'before,kind,limit,requestId' &&
    typeof entry.requestId === 'string' &&
    /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(entry.requestId) &&
    typeof entry.kind === 'string' &&
    ['login', 'operation', 'error'].includes(entry.kind) &&
    typeof entry.before === 'string' &&
    entry.before.length <= 64 &&
    Number.isFinite(Date.parse(entry.before)) &&
    typeof entry.limit === 'number' &&
    Number.isInteger(entry.limit) &&
    entry.limit >= 1 &&
    entry.limit <= 1000
  )
}
export function loadPendingCleanups(
  storage: JournalStorage,
  scope: CleanupScope,
): LogCleanupRequest[] {
  const start = prefix(scope),
    entries: LogCleanupRequest[] = []
  for (let index = 0; index < storage.length; index++) {
    const key = storage.key(index)
    if (!key?.startsWith(start)) continue
    const text = storage.getItem(key)
    if (text === null) continue
    let entry: unknown
    try {
      if (text.length > 2048) throw Error()
      entry = JSON.parse(text)
    } catch {
      throw new Error('本机待确认清理记录无效，请人工核对')
    }
    if (!valid(entry) || key !== start + entry.requestId)
      throw new Error('本机待确认清理记录无效，请人工核对')
    entries.push({ ...entry })
  }
  return entries.sort((a, b) => a.requestId.localeCompare(b.requestId))
}
export function savePendingCleanup(
  storage: JournalStorage,
  scope: CleanupScope,
  input: LogCleanupRequest,
) {
  if (!valid(input)) throw new Error('清理请求无效')
  const existing = loadPendingCleanups(storage, scope).find(
    (entry) => entry.requestId === input.requestId,
  )
  if (
    existing &&
    (existing.kind !== input.kind ||
      existing.before !== input.before ||
      existing.limit !== input.limit)
  )
    throw new Error('不能修改待确认清理请求的原条件')
  storage.setItem(prefix(scope) + input.requestId, JSON.stringify(input))
}
export function completePendingCleanup(
  storage: JournalStorage,
  scope: CleanupScope,
  requestId: string,
) {
  storage.removeItem(prefix(scope) + requestId)
}
export function parseCleanupResult(value: unknown, request: LogCleanupRequest): LogCleanupResult {
  const result = value as Partial<LogCleanupResult> | null
  if (
    !result ||
    result.requestId !== request.requestId ||
    result.kind !== request.kind ||
    typeof result.before !== 'string' ||
    Date.parse(result.before) !== Date.parse(request.before) ||
    typeof result.deletedCount !== 'number' ||
    !Number.isInteger(result.deletedCount) ||
    result.deletedCount < 0 ||
    result.deletedCount > request.limit ||
    typeof result.hasMore !== 'boolean' ||
    typeof result.completedAt !== 'string' ||
    !Number.isFinite(Date.parse(result.completedAt))
  )
    throw new Error('清理响应无效，原请求结果尚未确认')
  return result as LogCleanupResult
}
