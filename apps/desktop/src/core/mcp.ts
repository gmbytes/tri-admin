import { normalizeEndpoint } from './endpoint'
import { objectId, requestId } from './open-access'
export type McpKind =
  'start' | 'stop' | 'restart' | 'refresh' | 'call' | 'cancel' | 'orphan.confirm'
export type McpTransport =
  | { kind: 'stdio'; capabilityId: string }
  | { kind: 'http'; profileId: string; profileVersion: number; allowedTools: string[] }
export interface McpRuntime {
  id: string
  state: string
  transportKind: 'stdio' | 'http'
  pid: number | null
  releaseConfirmed: boolean
  updatedAt: string
  errorCode: string | null
}
export interface McpConnection {
  id: string
  ownerId: string
  requestId: string
  name: string
  transport: McpTransport
  capabilityCurrent: boolean
  enabled: boolean
  version: number
  runtime: McpRuntime | null
  createdAt: string
  updatedAt: string
  deletedAt: string | null
}
export interface McpAction {
  id: string
  actorId: string
  ownerId: string
  connectionId: string
  requestId: string
  kind: McpKind
  status:
    | 'pending'
    | 'running'
    | 'succeeded'
    | 'failed'
    | 'cancelled'
    | 'unknown'
    | 'inputRequired'
    | 'remoteTask'
  connectionVersion: number
  instanceId: string | null
  targetActionId: string | null
  name: string | null
  argumentsSha256: string | null
  errorCode: string | null
  resultKind: 'complete' | 'inputRequired' | 'task' | null
  resultSize: number | null
  resultSha256: string | null
  createdAt: string
  finishedAt: string | null
  cancelRequested: boolean
}
export interface McpTool {
  name: string
  description: string
  inputSchema: Record<string, unknown>
  raw: Record<string, unknown>
}
export interface McpCapability {
  id: string
  label: string
  allowedTools: string[]
}
export interface McpResult {
  content: string
  size: number
  sha256: string
  kind: 'complete' | 'inputRequired' | 'task'
}
export interface McpOperation {
  requestId: string
  kind: McpKind | 'create'
  connectionId: string | null
  targetActionId: string | null
  version: number | null
  createdAt: string
}
const kinds = ['start', 'stop', 'restart', 'refresh', 'call', 'cancel', 'orphan.confirm'] as const
const maxVersion = 9_007_199_254_740_990
const runtimeStates = [
  'starting',
  'running',
  'stopping',
  'stopped',
  'failed',
  'orphaned',
  'unconfirmed',
  'disconnected',
  'retired',
]
function invalid(): never {
  throw new Error('MCP 响应或恢复记录无效')
}
function object(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === 'object' && !Array.isArray(v)
}
function exact(v: Record<string, unknown>, keys: string[]) {
  return Object.keys(v).sort().join('|') === [...keys].sort().join('|')
}
function integer(v: unknown, min = 1, max = maxVersion): v is number {
  return typeof v === 'number' && Number.isSafeInteger(v) && v >= min && v <= max
}
function text(v: unknown, max: number): v is string {
  return (
    typeof v === 'string' && !!v.trim() && [...v].length <= max && !/[\u0000-\u001f\u007f]/.test(v)
  )
}
function date(v: unknown): v is string {
  return (
    typeof v === 'string' &&
    /^\d{4}-\d\d-\d\dT.+(?:Z|[+-]\d\d:\d\d)$/.test(v) &&
    Number.isFinite(Date.parse(v))
  )
}
function hash(v: unknown): v is string {
  return typeof v === 'string' && /^[a-f0-9]{64}$/.test(v)
}
function toolNames(v: unknown, maxBytes = 16384): v is string[] {
  return (
    Array.isArray(v) &&
    v.length > 0 &&
    v.length <= 128 &&
    v.every((name) => text(name, 128)) &&
    new Set(v).size === v.length &&
    new TextEncoder().encode(JSON.stringify(v)).length <= maxBytes
  )
}
export function parseToolLines(raw: string): string[] {
  const names = raw.split(/\r?\n/).filter((value) => value.trim().length > 0)
  if (!toolNames(names)) invalid()
  return names
}
function jsonTree(v: unknown) {
  const stack: Array<[unknown, number]> = [[v, 0]]
  let nodes = 0
  while (stack.length) {
    const [value, depth] = stack.pop()!
    if (++nodes > 4096 || depth > 32 || (typeof value === 'number' && !Number.isFinite(value)))
      invalid()
    if (Array.isArray(value))
      stack.push(...value.map((item) => [item, depth + 1] as [unknown, number]))
    else if (object(value))
      stack.push(...Object.values(value).map((item) => [item, depth + 1] as [unknown, number]))
  }
}
export function parseTransport(v: unknown): McpTransport {
  if (!object(v)) invalid()
  if (v.kind === 'stdio') {
    if (
      !exact(v, ['kind', 'capabilityId']) ||
      typeof v.capabilityId !== 'string' ||
      !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(v.capabilityId) ||
      v.capabilityId.length > 64
    )
      invalid()
    return { kind: 'stdio', capabilityId: v.capabilityId }
  }
  if (v.kind === 'http') {
    if (
      !exact(v, ['kind', 'profileId', 'profileVersion', 'allowedTools']) ||
      !objectId(v.profileId) ||
      !integer(v.profileVersion) ||
      !toolNames(v.allowedTools)
    )
      invalid()
    return {
      kind: 'http',
      profileId: v.profileId,
      profileVersion: v.profileVersion,
      allowedTools: [...v.allowedTools],
    }
  }
  return invalid()
}
function runtime(v: unknown): McpRuntime {
  if (
    !object(v) ||
    !exact(v, [
      'id',
      'state',
      'transportKind',
      'pid',
      'releaseConfirmed',
      'updatedAt',
      'errorCode',
    ]) ||
    !objectId(v.id) ||
    typeof v.state !== 'string' ||
    !runtimeStates.includes(v.state) ||
    !['stdio', 'http'].includes(String(v.transportKind)) ||
    !(v.pid === null || integer(v.pid, 1, 4294967295)) ||
    (v.transportKind === 'http' && v.pid !== null) ||
    typeof v.releaseConfirmed !== 'boolean' ||
    !date(v.updatedAt) ||
    !(v.errorCode === null || text(v.errorCode, 100))
  )
    invalid()
  return v as unknown as McpRuntime
}
export function parseConnection(v: unknown): McpConnection {
  if (
    !object(v) ||
    !exact(v, [
      'id',
      'ownerId',
      'requestId',
      'name',
      'transport',
      'capabilityCurrent',
      'enabled',
      'version',
      'runtime',
      'createdAt',
      'updatedAt',
      'deletedAt',
    ]) ||
    !objectId(v.id) ||
    !objectId(v.ownerId) ||
    !requestId(v.requestId) ||
    !text(v.name, 64) ||
    typeof v.capabilityCurrent !== 'boolean' ||
    typeof v.enabled !== 'boolean' ||
    !integer(v.version) ||
    !date(v.createdAt) ||
    !date(v.updatedAt) ||
    !(v.deletedAt === null || date(v.deletedAt))
  )
    invalid()
  return {
    ...v,
    transport: parseTransport(v.transport),
    runtime: v.runtime === null ? null : runtime(v.runtime),
  } as unknown as McpConnection
}
export function parseAction(v: unknown): McpAction {
  if (
    !object(v) ||
    !exact(v, [
      'id',
      'actorId',
      'ownerId',
      'connectionId',
      'requestId',
      'kind',
      'status',
      'connectionVersion',
      'instanceId',
      'targetActionId',
      'name',
      'argumentsSha256',
      'errorCode',
      'resultKind',
      'resultSize',
      'resultSha256',
      'createdAt',
      'finishedAt',
      'cancelRequested',
    ]) ||
    ![v.id, v.actorId, v.ownerId, v.connectionId].every(objectId) ||
    !requestId(v.requestId) ||
    !kinds.includes(v.kind as McpKind) ||
    ![
      'pending',
      'running',
      'succeeded',
      'failed',
      'cancelled',
      'unknown',
      'inputRequired',
      'remoteTask',
    ].includes(String(v.status)) ||
    !integer(v.connectionVersion) ||
    !(v.instanceId === null || objectId(v.instanceId)) ||
    !(v.targetActionId === null || objectId(v.targetActionId)) ||
    !(v.name === null || text(v.name, 128)) ||
    !(v.argumentsSha256 === null || hash(v.argumentsSha256)) ||
    !(v.errorCode === null || text(v.errorCode, 100)) ||
    !(
      v.resultKind === null || ['complete', 'inputRequired', 'task'].includes(String(v.resultKind))
    ) ||
    !(v.resultSize === null || integer(v.resultSize, 0, 512 * 1024)) ||
    !(v.resultSha256 === null || hash(v.resultSha256)) ||
    !date(v.createdAt) ||
    !(v.finishedAt === null || date(v.finishedAt)) ||
    typeof v.cancelRequested !== 'boolean'
  )
    invalid()
  return v as unknown as McpAction
}
export function parseCapabilities(v: unknown): McpCapability[] {
  if (!Array.isArray(v) || v.length > 16) invalid()
  const ids = new Set<string>()
  return v.map((row) => {
    if (
      !object(row) ||
      !exact(row, ['id', 'label', 'allowedTools']) ||
      typeof row.id !== 'string' ||
      !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(row.id) ||
      row.id.length > 64 ||
      !text(row.label, 80) ||
      !toolNames(row.allowedTools, 65536) ||
      ids.has(row.id)
    )
      invalid()
    ids.add(row.id)
    return { id: row.id, label: row.label, allowedTools: [...row.allowedTools] }
  })
}
export function parseTools(v: unknown): McpTool[] {
  if (
    !Array.isArray(v) ||
    v.length > 128 ||
    new TextEncoder().encode(JSON.stringify(v)).length > 256 * 1024
  )
    invalid()
  jsonTree(v)
  const names = new Set<string>()
  return v.map((row) => {
    if (
      !object(row) ||
      !text(row.name, 128) ||
      names.has(row.name) ||
      !object(row.inputSchema) ||
      row.inputSchema.type !== 'object' ||
      !(row.description === undefined || typeof row.description === 'string')
    )
      invalid()
    names.add(row.name)
    return {
      name: row.name,
      description: typeof row.description === 'string' ? row.description : '',
      inputSchema: row.inputSchema,
      raw: row,
    }
  })
}
export function validateArguments(raw: string): string {
  if (typeof raw !== 'string' || new TextEncoder().encode(raw).length > 64 * 1024) invalid()
  let value: unknown
  try {
    value = JSON.parse(raw)
  } catch {
    return invalid()
  }
  if (!object(value)) invalid()
  jsonTree(value)
  return raw
}
export function buildCallDocument(
  request: string,
  version: number,
  name: string,
  argumentsJson: string,
): string {
  if (!requestId(request) || !integer(version) || !text(name, 128)) invalid()
  const raw = validateArguments(argumentsJson)
  return `{"requestId":${JSON.stringify(request)},"version":${version},"name":${JSON.stringify(name)},"arguments":${raw}}`
}
export function parseResult(v: unknown): McpResult {
  if (
    !object(v) ||
    !exact(v, ['content', 'size', 'sha256', 'kind']) ||
    typeof v.content !== 'string' ||
    !integer(v.size, 0, 512 * 1024) ||
    !hash(v.sha256) ||
    !['complete', 'inputRequired', 'task'].includes(String(v.kind)) ||
    new TextEncoder().encode(v.content).length !== v.size
  )
    invalid()
  try {
    if (!object(JSON.parse(v.content))) invalid()
  } catch {
    invalid()
  }
  return v as unknown as McpResult
}
export async function verifyResult(v: McpResult) {
  parseResult(v)
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(v.content))
  if (
    Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('') !==
    v.sha256
  )
    invalid()
}
export function resultLabel(v: McpResult) {
  if (v.kind === 'task') return '收到远端任务'
  if (v.kind === 'inputRequired') return '需要后续输入'
  const result = JSON.parse(v.content) as Record<string, unknown>
  if (
    object(result.structuredContent) &&
    object(result.structuredContent.error) &&
    result.structuredContent.error.effect === 'unknown'
  )
    return '远端效果未确认'
  return result.isError === true ? '工具返回错误' : '收到完整响应'
}
function operation(v: unknown): McpOperation {
  if (
    !object(v) ||
    !exact(v, ['requestId', 'kind', 'connectionId', 'targetActionId', 'version', 'createdAt']) ||
    !requestId(v.requestId) ||
    !['create', ...kinds].includes(String(v.kind)) ||
    !date(v.createdAt) ||
    (v.kind === 'create'
      ? v.connectionId !== null || v.targetActionId !== null || v.version !== null
      : !objectId(v.connectionId)) ||
    (v.kind === 'cancel'
      ? !objectId(v.targetActionId) || v.version !== null
      : v.targetActionId !== null) ||
    (v.kind !== 'create' && v.kind !== 'cancel' && !integer(v.version))
  )
    invalid()
  return v as unknown as McpOperation
}
export function verifyMcpReceipt(
  pending: McpOperation,
  value: unknown,
  actor: string,
): McpConnection | McpAction {
  const op = operation(pending)
  if (!objectId(actor)) invalid()
  if (op.kind === 'create') {
    const row = parseConnection(value)
    if (row.requestId !== op.requestId || row.ownerId !== actor) invalid()
    return row
  }
  const row = parseAction(value),
    expected =
      op.version === null
        ? null
        : op.version + (['start', 'stop', 'restart'].includes(op.kind) ? 1 : 0)
  if (
    row.requestId !== op.requestId ||
    row.actorId !== actor ||
    row.connectionId !== op.connectionId ||
    row.kind !== op.kind ||
    (expected !== null && row.connectionVersion !== expected) ||
    (op.kind === 'cancel' && row.targetActionId !== op.targetActionId)
  )
    invalid()
  return row
}
export class McpRecoveryStore {
  private key: string
  constructor(
    private storage: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>,
    endpoint: string,
    actor: string,
  ) {
    if (!objectId(actor)) invalid()
    this.key = `tri-admin.mcp.v1:${encodeURIComponent(normalizeEndpoint(endpoint))}:${actor}`
  }
  read(): McpOperation[] {
    const raw = this.storage.getItem(this.key)
    if (raw === null) return []
    if (raw.length > 32768) invalid()
    let value: unknown
    try {
      value = JSON.parse(raw)
    } catch {
      return invalid()
    }
    if (
      !object(value) ||
      !exact(value, ['formatVersion', 'operations']) ||
      value.formatVersion !== 1 ||
      !Array.isArray(value.operations) ||
      value.operations.length > 32
    )
      invalid()
    const rows = value.operations.map(operation)
    if (new Set(rows.map((row) => row.requestId)).size !== rows.length) invalid()
    return rows
  }
  add(value: McpOperation) {
    const row = operation(value),
      rows = this.read(),
      old = rows.find((item) => item.requestId === row.requestId)
    if (old) {
      if (JSON.stringify(old) !== JSON.stringify(row)) invalid()
      return
    }
    if (rows.length >= 32) invalid()
    this.storage.setItem(this.key, JSON.stringify({ formatVersion: 1, operations: [...rows, row] }))
  }
  clear() {
    this.storage.removeItem(this.key)
  }
  remove(request: string) {
    if (!requestId(request)) invalid()
    const rows = this.read().filter((row) => row.requestId !== request)
    if (rows.length)
      this.storage.setItem(this.key, JSON.stringify({ formatVersion: 1, operations: rows }))
    else this.storage.removeItem(this.key)
  }
}
