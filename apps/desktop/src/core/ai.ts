import { normalizeEndpoint } from './endpoint'
import { objectId, requestId } from './open-access'
export type AiState = 'pending' | 'running' | 'succeeded' | 'failed' | 'cancelled' | 'unknown'
export type AiResponseStatus = 'completed' | 'failed' | 'incomplete' | 'cancelled'
export interface AiModel {
  id: string
  label: string
  maxOutputTokens: number
}
export interface AiProvider {
  id: string
  name: string
  profileId: string
  profileVersion: number
  bindingCurrent: boolean
  models: AiModel[]
  enabled: boolean
  version: number
  createdAt: string
  updatedAt: string
}
export interface AiApprovedModel extends AiModel {
  providerId: string
  providerName: string
  providerVersion: number
}
export interface AiRun {
  id: string
  ownerId: string
  requestId: string
  providerId: string
  providerVersion: number
  model: string
  state: AiState
  version: number
  mayHaveBeenSubmitted: boolean
  hasResponse: boolean
  cancelRequested: boolean
  cancelRequestId: string | null
  cancelConfirmed: boolean
  responseStatus: AiResponseStatus | null
  errorCode: string | null
  createdAt: string
  updatedAt: string
  finishedAt: string | null
  expiresAt: string
}
export interface AiContent {
  input: string
  instructions: string | null
  text: string
  sha256: string
  responseStatus: AiResponseStatus | null
  refused: boolean
  sequence: string | null
}
export interface AiOperation {
  requestId: string
  kind: 'run.create' | 'run.cancel'
  runId: string | null
  version: number | null
  createdAt: string
}
const maxVersion = 9_007_199_254_740_990
const states = ['pending', 'running', 'succeeded', 'failed', 'cancelled', 'unknown']
const responseStatuses = ['completed', 'failed', 'incomplete', 'cancelled']
function invalid(): never {
  throw new Error('AI 数据或恢复记录无效')
}
function object(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === 'object' && !Array.isArray(value)
}
function exact(value: Record<string, unknown>, keys: string[]) {
  if (Object.keys(value).sort().join('|') !== [...keys].sort().join('|')) invalid()
}
function text(value: unknown, max: number): value is string {
  return (
    typeof value === 'string' &&
    value.trim().length > 0 &&
    value.length <= max &&
    !/[\u0000-\u001f\u007f]/u.test(value)
  )
}
function integer(value: unknown, min = 1, max = maxVersion): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= min && value <= max
}
function timestamp(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,7})?Z$/u.test(value) &&
    Number.isFinite(Date.parse(value))
  )
}
function modelId(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    value.length > 0 &&
    value.length <= 160 &&
    /^[A-Za-z0-9_.:/-]+$/u.test(value)
  )
}
function responseStatus(value: unknown): value is AiResponseStatus | null {
  return value === null || (typeof value === 'string' && responseStatuses.includes(value))
}
function nullableText(value: unknown, max: number): value is string | null {
  return value === null || text(value, max)
}
function bytes(value: string): Uint8Array<ArrayBuffer> {
  return new TextEncoder().encode(value)
}
export function parseAiModel(value: unknown): AiModel {
  if (!object(value)) return invalid()
  exact(value, ['id', 'label', 'maxOutputTokens'])
  if (!modelId(value.id) || !text(value.label, 80) || !integer(value.maxOutputTokens, 1, 32768))
    return invalid()
  return { id: value.id, label: value.label, maxOutputTokens: value.maxOutputTokens }
}
export function parseAiProvider(value: unknown): AiProvider {
  if (!object(value)) return invalid()
  exact(value, [
    'id',
    'name',
    'profileId',
    'profileVersion',
    'bindingCurrent',
    'models',
    'enabled',
    'version',
    'createdAt',
    'updatedAt',
  ])
  if (
    !objectId(value.id) ||
    !text(value.name, 80) ||
    !objectId(value.profileId) ||
    !integer(value.profileVersion) ||
    typeof value.bindingCurrent !== 'boolean' ||
    !Array.isArray(value.models) ||
    value.models.length < 1 ||
    value.models.length > 32 ||
    typeof value.enabled !== 'boolean' ||
    !integer(value.version) ||
    !timestamp(value.createdAt) ||
    !timestamp(value.updatedAt)
  )
    return invalid()
  const models = value.models.map(parseAiModel)
  if (new Set(models.map((model) => model.id)).size !== models.length) return invalid()
  return {
    id: value.id,
    name: value.name,
    profileId: value.profileId,
    profileVersion: value.profileVersion,
    bindingCurrent: value.bindingCurrent,
    models,
    enabled: value.enabled,
    version: value.version,
    createdAt: value.createdAt,
    updatedAt: value.updatedAt,
  }
}
export function parseAiApprovedModel(value: unknown): AiApprovedModel {
  if (!object(value)) return invalid()
  exact(value, ['providerId', 'providerName', 'providerVersion', 'id', 'label', 'maxOutputTokens'])
  if (
    !objectId(value.providerId) ||
    !text(value.providerName, 80) ||
    !integer(value.providerVersion)
  )
    return invalid()
  const model = parseAiModel({
    id: value.id,
    label: value.label,
    maxOutputTokens: value.maxOutputTokens,
  })
  return {
    ...model,
    providerId: value.providerId,
    providerName: value.providerName,
    providerVersion: value.providerVersion,
  }
}
export function parseAiRun(value: unknown): AiRun {
  if (!object(value)) return invalid()
  exact(value, [
    'id',
    'ownerId',
    'requestId',
    'providerId',
    'providerVersion',
    'model',
    'state',
    'version',
    'mayHaveBeenSubmitted',
    'hasResponse',
    'cancelRequested',
    'cancelRequestId',
    'cancelConfirmed',
    'responseStatus',
    'errorCode',
    'createdAt',
    'updatedAt',
    'finishedAt',
    'expiresAt',
  ])
  if (
    !objectId(value.id) ||
    !objectId(value.ownerId) ||
    !requestId(value.requestId) ||
    !objectId(value.providerId) ||
    !integer(value.providerVersion) ||
    !modelId(value.model) ||
    typeof value.state !== 'string' ||
    !states.includes(value.state) ||
    !integer(value.version) ||
    typeof value.mayHaveBeenSubmitted !== 'boolean' ||
    typeof value.hasResponse !== 'boolean' ||
    typeof value.cancelRequested !== 'boolean' ||
    (value.cancelRequestId !== null && !requestId(value.cancelRequestId)) ||
    typeof value.cancelConfirmed !== 'boolean' ||
    !responseStatus(value.responseStatus) ||
    !nullableText(value.errorCode, 80) ||
    !timestamp(value.createdAt) ||
    !timestamp(value.updatedAt) ||
    (value.finishedAt !== null && !timestamp(value.finishedAt)) ||
    !timestamp(value.expiresAt)
  )
    return invalid()
  if (
    value.cancelRequested !== (value.cancelRequestId !== null) ||
    (value.cancelConfirmed &&
      (value.state !== 'cancelled' ||
        (value.mayHaveBeenSubmitted && value.responseStatus !== 'cancelled'))) ||
    (value.state === 'succeeded' &&
      (value.responseStatus !== 'completed' || value.errorCode !== null)) ||
    (value.state === 'cancelled' && !value.cancelConfirmed)
  )
    return invalid()
  return value as unknown as AiRun
}
export async function verifyAiContent(value: unknown): Promise<AiContent> {
  if (!object(value)) return invalid()
  exact(value, ['input', 'instructions', 'text', 'sha256', 'responseStatus', 'refused', 'sequence'])
  if (
    typeof value.input !== 'string' ||
    bytes(value.input).length > 65536 ||
    (value.instructions !== null &&
      (typeof value.instructions !== 'string' || bytes(value.instructions).length > 16384)) ||
    typeof value.text !== 'string' ||
    bytes(value.text).length > 1024 * 1024 ||
    typeof value.sha256 !== 'string' ||
    !/^[a-f0-9]{64}$/u.test(value.sha256) ||
    !responseStatus(value.responseStatus) ||
    typeof value.refused !== 'boolean' ||
    (value.sequence !== null &&
      (typeof value.sequence !== 'string' ||
        !/^(?:0|[1-9][0-9]{0,19})$/u.test(value.sequence) ||
        BigInt(value.sequence) > 18446744073709551615n))
  )
    return invalid()
  const digest = await crypto.subtle.digest('SHA-256', bytes(value.text))
  const sha256 = Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('')
  if (sha256 !== value.sha256) return invalid()
  return value as unknown as AiContent
}
export function parseAiOperation(value: unknown): AiOperation {
  if (!object(value)) return invalid()
  exact(value, ['requestId', 'kind', 'runId', 'version', 'createdAt'])
  if (
    !requestId(value.requestId) ||
    typeof value.kind !== 'string' ||
    !['run.create', 'run.cancel'].includes(value.kind) ||
    !timestamp(value.createdAt)
  )
    return invalid()
  if (value.kind === 'run.create') {
    if (value.runId !== null || value.version !== null) return invalid()
  } else if (!objectId(value.runId) || !integer(value.version)) return invalid()
  return value as unknown as AiOperation
}
export function aiRecoveryKey(endpoint: string, accountId: string): string {
  if (!objectId(accountId)) return invalid()
  return `tri-admin.ai-recovery.v1:${encodeURIComponent(normalizeEndpoint(endpoint))}:${accountId}`
}
export function verifyAiReceipt(operation: AiOperation, value: unknown, actor: string): AiRun {
  const op = parseAiOperation(operation),
    row = parseAiRun(value)
  if (
    !objectId(actor) ||
    row.ownerId !== actor ||
    (op.kind === 'run.create'
      ? row.requestId !== op.requestId
      : row.id !== op.runId || row.cancelRequestId !== op.requestId || !row.cancelRequested)
  )
    invalid()
  return row
}
export class AiRecoveryStore {
  private key: string
  constructor(
    private storage: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>,
    endpoint: string,
    actor: string,
  ) {
    this.key = aiRecoveryKey(endpoint, actor)
  }
  read(): AiOperation[] {
    const raw = this.storage.getItem(this.key)
    if (raw === null) return []
    if (raw.length > 32768) return invalid()
    let value: unknown
    try {
      value = JSON.parse(raw)
    } catch {
      return invalid()
    }
    if (!object(value)) return invalid()
    exact(value, ['formatVersion', 'operations'])
    if (
      value.formatVersion !== 1 ||
      !Array.isArray(value.operations) ||
      value.operations.length > 32
    )
      return invalid()
    const rows = value.operations.map(parseAiOperation)
    if (new Set(rows.map((row) => row.requestId)).size !== rows.length) invalid()
    return rows
  }
  add(value: AiOperation) {
    const row = parseAiOperation(value),
      rows = this.read(),
      old = rows.find((item) => item.requestId === row.requestId)
    if (old) {
      if (JSON.stringify(old) !== JSON.stringify(row)) invalid()
      return
    }
    if (rows.length >= 32) return invalid()
    this.storage.setItem(this.key, JSON.stringify({ formatVersion: 1, operations: [...rows, row] }))
  }
  remove(request: string) {
    if (!requestId(request)) invalid()
    const rows = this.read().filter((row) => row.requestId !== request)
    if (rows.length)
      this.storage.setItem(this.key, JSON.stringify({ formatVersion: 1, operations: rows }))
    else this.storage.removeItem(this.key)
  }
}
