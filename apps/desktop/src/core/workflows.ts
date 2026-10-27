import { normalizeEndpoint } from './endpoint'
import { objectId, requestId } from './open-access'
export type WorkflowMode = 'project' | 'feature' | 'mcpGuide'
export type WorkflowState = 'pending' | 'running' | 'succeeded' | 'failed' | 'cancelled' | 'unknown'
export interface WorkflowStep {
  id: string
  title: string
  prompt: string
  dependsOn: string[]
  expected: 'text' | 'analysis'
}
export interface WorkflowPlan {
  mode: WorkflowMode
  steps: WorkflowStep[]
}
export interface WorkflowInputs {
  requirements: string
  scene: string
  constraints: string
}
export interface AnalysisField {
  name: string
  label: string
  kind: 'string' | 'integer' | 'number' | 'boolean' | 'dateTime' | 'objectId' | 'json'
  required: boolean
}
export interface Analysis {
  summary: string
  modules: { key: string; label: string; description: string; fields: AnalysisField[] }[]
  risks: string[]
  questions: string[]
}
export interface WorkflowNode {
  id: string
  state: WorkflowState
  aiRunId: string | null
  errorCode: string | null
}
export interface WorkflowRun {
  id: string
  ownerId: string
  requestId: string
  planId: string | null
  planVersion: number | null
  mode: WorkflowMode
  state: WorkflowState
  version: number
  completedSteps: number
  stepCount: number
  nodes: WorkflowNode[]
  cancelRequested: boolean
  cancelConfirmed: boolean
  errorCode: string | null
  createdAt: string
  updatedAt: string
  finishedAt: string | null
}
export interface WorkflowOperation {
  requestId: string
  kind:
    | 'plan.create'
    | 'run.create'
    | 'run.analyze'
    | 'run.cancel'
    | 'run.export'
    | 'session.create'
    | 'message.create'
  targetId: string | null
  version: number | null
  createdAt: string
}
export interface WorkflowPlanRecord {
  id: string
  ownerId: string
  requestId: string
  name: string
  mode: WorkflowMode
  stepCount: number
  version: number
  createdAt: string
  updatedAt: string
  deletedAt: string | null
}
export interface WorkflowDefinition {
  planId: string
  version: number
  name: string
  sha256: string
  plan: WorkflowPlan
}
export interface WorkflowSelection {
  providerId: string | null
  providerVersion: number | null
  model: string | null
  maxOutputTokens: number
  stream: boolean
  mode: WorkflowMode
  planId: string | null
  planVersion: number | null
}
export interface WorkflowSession {
  id: string
  ownerId: string
  requestId: string
  name: string
  version: number
  currentMessageId: string | null
  messageCount: number
  expiresAt: string
  createdAt: string
  updatedAt: string
  deletedAt: string | null
}
export interface WorkflowSessionContent {
  version: number
  selection: WorkflowSelection
  draft: WorkflowInputs | null
  currentMessageId: string | null
  currentNode: string | null
}
export interface WorkflowMessage {
  id: string
  sessionId: string
  requestId: string
  parentMessageId: string | null
  runId: string
  sessionVersion: number
  createdAt: string
}
export interface WorkflowExport {
  id: string
  requestId: string
  runId: string
  runVersion: number
  size: number
  sha256: string
  fileName: string
  expiresAt: string
  createdAt: string
}
const modes = ['project', 'feature', 'mcpGuide'],
  states = ['pending', 'running', 'succeeded', 'failed', 'cancelled', 'unknown']
function invalid(): never {
  throw new Error('工作流数据或恢复记录无效')
}
function obj(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === 'object' && !Array.isArray(v)
}
function exact(v: Record<string, unknown>, keys: string[]) {
  if (Object.keys(v).sort().join('|') !== [...keys].sort().join('|')) invalid()
}
function str(v: unknown, limit: number, nonempty = false): v is string {
  return (
    typeof v === 'string' &&
    (!nonempty || !!v.trim()) &&
    new TextEncoder().encode(v).length <= limit
  )
}
function label(v: unknown, max: number): v is string {
  return (
    typeof v === 'string' && !!v.trim() && [...v].length <= max && !/[\u0000-\u001f\u007f]/u.test(v)
  )
}
function int(v: unknown, min = 1, max = 9_007_199_254_740_990): v is number {
  return typeof v === 'number' && Number.isSafeInteger(v) && v >= min && v <= max
}
function date(v: unknown): v is string {
  return (
    typeof v === 'string' &&
    /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,7})?Z$/u.test(v) &&
    Number.isFinite(Date.parse(v))
  )
}
function stepId(v: unknown): v is string {
  return typeof v === 'string' && /^[a-z][a-z0-9_-]{0,31}$/u.test(v)
}
function mode(v: unknown): v is WorkflowMode {
  return typeof v === 'string' && modes.includes(v)
}
function state(v: unknown): v is WorkflowState {
  return typeof v === 'string' && states.includes(v)
}
function hash(v: unknown): v is string {
  return typeof v === 'string' && /^[a-f0-9]{64}$/u.test(v)
}
export function parseWorkflowPlanRecord(v: unknown): WorkflowPlanRecord {
  if (!obj(v)) return invalid()
  exact(v, [
    'id',
    'ownerId',
    'requestId',
    'name',
    'mode',
    'stepCount',
    'version',
    'createdAt',
    'updatedAt',
    'deletedAt',
  ])
  if (
    !objectId(v.id) ||
    !objectId(v.ownerId) ||
    !requestId(v.requestId) ||
    !label(v.name, 80) ||
    !mode(v.mode) ||
    !int(v.stepCount, 1, 16) ||
    !int(v.version) ||
    !date(v.createdAt) ||
    !date(v.updatedAt) ||
    (v.deletedAt !== null && !date(v.deletedAt))
  )
    return invalid()
  return v as unknown as WorkflowPlanRecord
}
export async function verifyWorkflowDefinition(v: unknown): Promise<WorkflowDefinition> {
  if (!obj(v)) return invalid()
  exact(v, ['planId', 'version', 'name', 'sha256', 'plan'])
  if (!objectId(v.planId) || !int(v.version) || !label(v.name, 80) || !hash(v.sha256))
    return invalid()
  const plan = parseWorkflowPlan(v.plan)
  const canonical = JSON.stringify({
    mode: plan.mode,
    steps: plan.steps.map((s) => ({
      dependsOn: s.dependsOn,
      expected: s.expected,
      id: s.id,
      prompt: s.prompt,
      title: s.title,
    })),
  })
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(canonical))
  if (
    Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, '0')).join('') !== v.sha256
  )
    return invalid()
  return { planId: v.planId, version: v.version, name: v.name, sha256: v.sha256, plan }
}
export function parseWorkflowSelection(v: unknown): WorkflowSelection {
  if (!obj(v)) return invalid()
  exact(v, [
    'providerId',
    'providerVersion',
    'model',
    'maxOutputTokens',
    'stream',
    'mode',
    'planId',
    'planVersion',
  ])
  if (
    v.providerId === null
      ? v.providerVersion !== null || v.model !== null
      : !objectId(v.providerId) || !int(v.providerVersion) || !label(v.model, 160)
  )
    return invalid()
  if (v.planId === null ? v.planVersion !== null : !objectId(v.planId) || !int(v.planVersion))
    return invalid()
  if (!int(v.maxOutputTokens, 1, 32768) || typeof v.stream !== 'boolean' || !mode(v.mode))
    return invalid()
  return v as unknown as WorkflowSelection
}
export function parseWorkflowSession(v: unknown): WorkflowSession {
  if (!obj(v)) return invalid()
  exact(v, [
    'id',
    'ownerId',
    'requestId',
    'name',
    'version',
    'currentMessageId',
    'messageCount',
    'expiresAt',
    'createdAt',
    'updatedAt',
    'deletedAt',
  ])
  if (
    !objectId(v.id) ||
    !objectId(v.ownerId) ||
    !requestId(v.requestId) ||
    !label(v.name, 80) ||
    !int(v.version) ||
    (v.currentMessageId !== null && !objectId(v.currentMessageId)) ||
    !int(v.messageCount, 0, 1000) ||
    !date(v.expiresAt) ||
    !date(v.createdAt) ||
    !date(v.updatedAt) ||
    (v.deletedAt !== null && !date(v.deletedAt))
  )
    return invalid()
  return v as unknown as WorkflowSession
}
export function parseWorkflowSessionContent(v: unknown): WorkflowSessionContent {
  if (!obj(v)) return invalid()
  exact(v, ['version', 'selection', 'draft', 'currentMessageId', 'currentNode'])
  if (
    !int(v.version) ||
    (v.currentMessageId !== null && !objectId(v.currentMessageId)) ||
    (v.currentNode !== null && !stepId(v.currentNode))
  )
    return invalid()
  return {
    version: v.version,
    selection: parseWorkflowSelection(v.selection),
    draft: v.draft === null ? null : parseWorkflowInputs(v.draft),
    currentMessageId: v.currentMessageId,
    currentNode: v.currentNode,
  }
}
export function parseWorkflowMessage(v: unknown): WorkflowMessage {
  if (!obj(v)) return invalid()
  exact(v, [
    'id',
    'sessionId',
    'requestId',
    'parentMessageId',
    'runId',
    'sessionVersion',
    'createdAt',
  ])
  if (
    !objectId(v.id) ||
    !objectId(v.sessionId) ||
    !requestId(v.requestId) ||
    (v.parentMessageId !== null && !objectId(v.parentMessageId)) ||
    !objectId(v.runId) ||
    !int(v.sessionVersion) ||
    !date(v.createdAt)
  )
    return invalid()
  return v as unknown as WorkflowMessage
}
export function parseWorkflowExport(v: unknown): WorkflowExport {
  if (!obj(v)) return invalid()
  exact(v, [
    'id',
    'requestId',
    'runId',
    'runVersion',
    'size',
    'sha256',
    'fileName',
    'expiresAt',
    'createdAt',
  ])
  if (
    !objectId(v.id) ||
    !requestId(v.requestId) ||
    !objectId(v.runId) ||
    !int(v.runVersion) ||
    !int(v.size, 1, 4 * 1024 * 1024) ||
    !hash(v.sha256) ||
    v.fileName !== `workflow-${v.runId}-v${v.runVersion}.md` ||
    !date(v.expiresAt) ||
    !date(v.createdAt)
  )
    return invalid()
  return v as unknown as WorkflowExport
}
export function parseWorkflowPlan(value: unknown): WorkflowPlan {
  if (!obj(value)) return invalid()
  exact(value, ['mode', 'steps'])
  if (
    !mode(value.mode) ||
    !Array.isArray(value.steps) ||
    value.steps.length < 1 ||
    value.steps.length > 16
  )
    return invalid()
  const steps = value.steps.map((v): WorkflowStep => {
    if (!obj(v)) return invalid()
    exact(v, ['id', 'title', 'prompt', 'dependsOn', 'expected'])
    if (
      !stepId(v.id) ||
      !label(v.title, 80) ||
      !str(v.prompt, 16384, true) ||
      !Array.isArray(v.dependsOn) ||
      v.dependsOn.length > 16 ||
      !v.dependsOn.every(stepId) ||
      !['text', 'analysis'].includes(String(v.expected))
    )
      return invalid()
    return v as unknown as WorkflowStep
  })
  const ids = new Set(steps.map((s) => s.id))
  if (
    ids.size !== steps.length ||
    steps.some(
      (s) =>
        new Set(s.dependsOn).size !== s.dependsOn.length ||
        s.dependsOn.some((d) => d === s.id || !ids.has(d)),
    )
  )
    return invalid()
  const done = new Set<string>()
  while (done.size < steps.length) {
    const next = steps.find((s) => !done.has(s.id) && s.dependsOn.every((d) => done.has(d)))
    if (!next) return invalid()
    done.add(next.id)
  }
  return { mode: value.mode, steps }
}
export function parseWorkflowInputs(v: unknown): WorkflowInputs {
  if (!obj(v)) return invalid()
  exact(v, ['requirements', 'scene', 'constraints'])
  if (!str(v.requirements, 16384, true) || !str(v.scene, 4096) || !str(v.constraints, 8192))
    return invalid()
  return v as unknown as WorkflowInputs
}
export function parseAnalysis(v: unknown): Analysis {
  if (!obj(v)) return invalid()
  exact(v, ['summary', 'modules', 'risks', 'questions'])
  if (
    !str(v.summary, 4096, true) ||
    !Array.isArray(v.modules) ||
    v.modules.length > 16 ||
    !Array.isArray(v.risks) ||
    v.risks.length > 32 ||
    !v.risks.every((x) => str(x, 2048, true)) ||
    !Array.isArray(v.questions) ||
    v.questions.length > 32 ||
    !v.questions.every((x) => str(x, 2048, true))
  )
    return invalid()
  const keys = new Set<string>()
  let fields = 0
  for (const m of v.modules) {
    if (!obj(m)) return invalid()
    exact(m, ['key', 'label', 'description', 'fields'])
    if (
      typeof m.key !== 'string' ||
      !/^[a-z][a-z0-9_]{0,47}$/u.test(m.key) ||
      keys.has(m.key) ||
      !label(m.label, 80) ||
      !str(m.description, 2048) ||
      !Array.isArray(m.fields) ||
      m.fields.length > 32 ||
      (fields += m.fields.length) > 256
    )
      return invalid()
    keys.add(m.key)
    const names = new Set<string>()
    for (const f of m.fields) {
      if (!obj(f)) return invalid()
      exact(f, ['name', 'label', 'kind', 'required'])
      if (
        typeof f.name !== 'string' ||
        !/^[A-Za-z][A-Za-z0-9_]{0,63}$/u.test(f.name) ||
        names.has(f.name) ||
        !label(f.label, 80) ||
        !['string', 'integer', 'number', 'boolean', 'dateTime', 'objectId', 'json'].includes(
          String(f.kind),
        ) ||
        typeof f.required !== 'boolean'
      )
        return invalid()
      names.add(f.name)
    }
  }
  return v as unknown as Analysis
}
export function parseWorkflowRun(v: unknown): WorkflowRun {
  if (!obj(v)) return invalid()
  exact(v, [
    'id',
    'ownerId',
    'requestId',
    'planId',
    'planVersion',
    'mode',
    'state',
    'version',
    'completedSteps',
    'stepCount',
    'nodes',
    'cancelRequested',
    'cancelConfirmed',
    'errorCode',
    'createdAt',
    'updatedAt',
    'finishedAt',
  ])
  if (
    !objectId(v.id) ||
    !objectId(v.ownerId) ||
    !requestId(v.requestId) ||
    (v.planId !== null && !objectId(v.planId)) ||
    (v.planVersion !== null && !int(v.planVersion)) ||
    (v.planId === null) !== (v.planVersion === null) ||
    !mode(v.mode) ||
    !state(v.state) ||
    !int(v.version) ||
    !int(v.completedSteps, 0, 16) ||
    !int(v.stepCount, 1, 16) ||
    !Array.isArray(v.nodes) ||
    v.nodes.length !== v.stepCount ||
    typeof v.cancelRequested !== 'boolean' ||
    typeof v.cancelConfirmed !== 'boolean' ||
    (v.errorCode !== null && !label(v.errorCode, 80)) ||
    !date(v.createdAt) ||
    !date(v.updatedAt) ||
    (v.finishedAt !== null && !date(v.finishedAt))
  )
    return invalid()
  for (const n of v.nodes) {
    if (!obj(n)) return invalid()
    exact(n, ['id', 'state', 'aiRunId', 'errorCode'])
    if (
      !stepId(n.id) ||
      !state(n.state) ||
      (n.aiRunId !== null && !objectId(n.aiRunId)) ||
      (n.errorCode !== null && !label(n.errorCode, 80))
    )
      return invalid()
  }
  if (
    new Set(v.nodes.map((n) => n.id)).size !== v.nodes.length ||
    v.completedSteps !== v.nodes.filter((n) => n.state === 'succeeded').length ||
    (v.state === 'succeeded' && (v.completedSteps !== v.stepCount || v.errorCode !== null)) ||
    v.cancelConfirmed !== (v.state === 'cancelled')
  )
    return invalid()
  return v as unknown as WorkflowRun
}
export function parseWorkflowOperation(v: unknown): WorkflowOperation {
  if (!obj(v)) return invalid()
  exact(v, ['requestId', 'kind', 'targetId', 'version', 'createdAt'])
  if (
    !requestId(v.requestId) ||
    !date(v.createdAt) ||
    ![
      'plan.create',
      'run.create',
      'run.analyze',
      'run.cancel',
      'run.export',
      'session.create',
      'message.create',
    ].includes(String(v.kind))
  )
    return invalid()
  const target = ['run.cancel', 'run.export', 'message.create'].includes(String(v.kind))
  if (target ? !objectId(v.targetId) || !int(v.version) : v.targetId !== null || v.version !== null)
    return invalid()
  return v as unknown as WorkflowOperation
}
export class WorkflowRecoveryStore {
  private key: string
  constructor(
    private storage: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>,
    endpoint: string,
    actor: string,
  ) {
    if (!objectId(actor)) invalid()
    this.key = `tri-admin.workflow-recovery.v1:${encodeURIComponent(normalizeEndpoint(endpoint))}:${actor}`
  }
  read(): WorkflowOperation[] {
    const raw = this.storage.getItem(this.key)
    if (raw === null) return []
    if (raw.length > 32768) return invalid()
    let v: unknown
    try {
      v = JSON.parse(raw)
    } catch {
      return invalid()
    }
    if (!obj(v)) return invalid()
    exact(v, ['formatVersion', 'operations'])
    if (v.formatVersion !== 1 || !Array.isArray(v.operations) || v.operations.length > 32)
      return invalid()
    const rows = v.operations.map(parseWorkflowOperation)
    if (new Set(rows.map((r) => r.requestId)).size !== rows.length) return invalid()
    return rows
  }
  add(value: WorkflowOperation) {
    const row = parseWorkflowOperation(value),
      rows = this.read(),
      old = rows.find((r) => r.requestId === row.requestId)
    if (old) {
      if (JSON.stringify(old) !== JSON.stringify(row)) invalid()
      return
    }
    if (rows.length >= 32) return invalid()
    this.storage.setItem(this.key, JSON.stringify({ formatVersion: 1, operations: [...rows, row] }))
  }
  remove(request: string) {
    if (!requestId(request)) invalid()
    const rows = this.read().filter((r) => r.requestId !== request)
    if (rows.length)
      this.storage.setItem(this.key, JSON.stringify({ formatVersion: 1, operations: rows }))
    else this.storage.removeItem(this.key)
  }
}
