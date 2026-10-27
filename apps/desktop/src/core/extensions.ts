import { normalizeEndpoint } from './endpoint'
import { objectId, requestId } from './open-access'
export interface ExtensionFile {
  path: string
  size: number
  mode: number
  sha256: string
  text: boolean
}
export interface SkillMeta {
  name: string
  description: string
  license: string | null
  compatibility: string | null
  metadata: Record<string, string>
  allowedTools: string | null
}
export interface PluginMcpContribution {
  id: string
  label: string
  capabilityId: string
  tools: string[]
}
export interface PluginToolBinding {
  name: string
  description: string
  operation: string
  inputSchema: Record<string, unknown>
  mappings: Record<string, string>
}
export interface PackageInfo {
  kind: 'skill' | 'plugin'
  name: string
  version: string | null
  description: string
  skills: { meta: SkillMeta; entryPath: string; sha256: string }[]
  mcp: PluginMcpContribution[]
  tools: PluginToolBinding[]
  files: ExtensionFile[]
  archiveSha256: string
  treeSha256: string
  archiveSize: number
  expandedSize: number
}
export interface Extension {
  id: string
  ownerId: string
  name: string
  kind: 'skill' | 'plugin'
  enabled: boolean
  version: number
  revision: number
  revisionId: string
  package: PackageInfo
  createdAt: string
  updatedAt: string
  deletedAt: string | null
}
export interface ExtensionRevision {
  id: string
  extensionId: string
  ownerId: string
  actorId: string
  requestId: string
  number: number
  package: PackageInfo
  createdAt: string
}
export interface Constraints {
  version: number
  content: string
  updatedAt: string
  actorId: string | null
}
export interface ExtensionPreview {
  path: string
  size: number
  sha256: string
  content: string
}
export interface ExtensionOperation {
  requestId: string
  kind: 'install' | 'revise'
  extensionId: string | null
  version: number | null
  createdAt: string
}
function invalid() {
  return new Error('扩展响应或恢复记录无效')
}
function object(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === 'object' && !Array.isArray(v)
}
function exact(v: Record<string, unknown>, keys: string[]) {
  return Object.keys(v).sort().join('|') === [...keys].sort().join('|')
}
function integer(v: unknown, min = 1, max = Number.MAX_SAFE_INTEGER): v is number {
  return typeof v === 'number' && Number.isSafeInteger(v) && v >= min && v <= max
}
function text(v: unknown, max: number, empty = false): v is string {
  return (
    typeof v === 'string' &&
    (empty || !!v.trim()) &&
    [...v].length <= max &&
    !/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/.test(v)
  )
}
function date(v: unknown): v is string {
  return (
    typeof v === 'string' &&
    /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/.test(v) &&
    Number.isFinite(Date.parse(v)) &&
    new Date(v).toISOString() === v
  )
}
function hash(v: unknown): v is string {
  return typeof v === 'string' && /^[a-f0-9]{64}$/.test(v)
}
export function extensionName(v: unknown): v is string {
  return typeof v === 'string' && /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(v) && v.length <= 64
}
export function pluginToolName(extension: string, revision: string, name: string): string {
  if (!objectId(extension) || !objectId(revision) || !/^[a-zA-Z0-9_.-]{1,64}$/.test(name))
    throw invalid()
  return `plugin.${extension}.${revision}.${name}`
}
export function extensionPath(v: unknown): v is string {
  if (
    typeof v !== 'string' ||
    !v ||
    new TextEncoder().encode(v).length > 240 ||
    v !== v.normalize('NFC') ||
    v.split('/').length > 16
  )
    return false
  return v
    .split('/')
    .every(
      (p) =>
        p &&
        p !== '.' &&
        p !== '..' &&
        new TextEncoder().encode(p).length <= 100 &&
        !/[<>:"\\|?*\u0000-\u001f\u007f-\u009f]/.test(p) &&
        !/[. ]$/.test(p) &&
        !['.git', '.svn', '.hg', '.ssh', '.env'].includes(p.toLowerCase()) &&
        !/^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\.|$)/i.test(p),
    )
}
function meta(v: unknown): SkillMeta {
  if (
    !object(v) ||
    !exact(v, ['name', 'description', 'license', 'compatibility', 'metadata', 'allowedTools']) ||
    !extensionName(v.name) ||
    !text(v.description, 1024) ||
    !(v.license === null || text(v.license, 256)) ||
    !(v.compatibility === null || text(v.compatibility, 500)) ||
    !(v.allowedTools === null || text(v.allowedTools, 2048)) ||
    !object(v.metadata) ||
    Object.keys(v.metadata).length > 64 ||
    Object.entries(v.metadata).some(([k, s]) => !text(k, 64) || !text(s, 1024))
  )
    throw invalid()
  return v as unknown as SkillMeta
}
function file(v: unknown): ExtensionFile {
  if (
    !object(v) ||
    !exact(v, ['path', 'size', 'mode', 'sha256', 'text']) ||
    !extensionPath(v.path) ||
    !integer(v.size, 0, 4 * 1024 * 1024) ||
    !integer(v.mode, 0, 511) ||
    !hash(v.sha256) ||
    typeof v.text !== 'boolean' ||
    (v.text && v.size > 256 * 1024)
  )
    throw invalid()
  return v as unknown as ExtensionFile
}
export function parsePackage(v: unknown): PackageInfo {
  if (
    !object(v) ||
    !exact(v, [
      'kind',
      'name',
      'version',
      'description',
      'skills',
      'mcp',
      'tools',
      'files',
      'archiveSha256',
      'treeSha256',
      'archiveSize',
      'expandedSize',
    ]) ||
    (v.kind !== 'skill' && v.kind !== 'plugin') ||
    !extensionName(v.name) ||
    !(v.version === null || text(v.version, 1024)) ||
    !text(v.description, 1024) ||
    !hash(v.archiveSha256) ||
    !hash(v.treeSha256) ||
    !integer(v.archiveSize, 1, 4 * 1024 * 1024) ||
    !integer(v.expandedSize, 0, 16 * 1024 * 1024) ||
    !Array.isArray(v.files) ||
    !v.files.length ||
    v.files.length > 256 ||
    !Array.isArray(v.skills) ||
    v.skills.length > 32 ||
    !Array.isArray(v.mcp) ||
    v.mcp.length > 16 ||
    !Array.isArray(v.tools) ||
    v.tools.length > 64
  )
    throw invalid()
  const files = v.files.map(file),
    paths = new Set(files.map((f) => f.path.toLowerCase()))
  if (paths.size !== files.length || files.reduce((n, f) => n + f.size, 0) !== v.expandedSize)
    throw invalid()
  for (const f of files) {
    const pieces = f.path.toLowerCase().split('/')
    for (let n = 1; n < pieces.length; n++)
      if (paths.has(pieces.slice(0, n).join('/'))) throw invalid()
  }
  const skills = v.skills.map((s) => {
    if (
      !object(s) ||
      !exact(s, ['meta', 'entryPath', 'sha256']) ||
      !extensionPath(s.entryPath) ||
      !hash(s.sha256) ||
      !files.some((f) => f.path === s.entryPath && f.sha256 === s.sha256)
    )
      throw invalid()
    return { meta: meta(s.meta), entryPath: s.entryPath, sha256: s.sha256 }
  })
  if (
    new Set(skills.map((s) => s.entryPath.toLowerCase())).size !== skills.length ||
    (v.kind === 'skill' &&
      (skills.length !== 1 ||
        skills[0]?.entryPath !== 'SKILL.md' ||
        skills[0]?.meta.name !== v.name))
  )
    throw invalid()
  const contributionNames = new Set<string>(),
    toolNames = new Set<string>()
  for (const m of v.mcp) {
    if (
      !object(m) ||
      !exact(m, ['id', 'label', 'capabilityId', 'tools']) ||
      !extensionName(m.id) ||
      !extensionName(m.capabilityId) ||
      !text(m.label, 80) ||
      !Array.isArray(m.tools) ||
      m.tools.length > 64 ||
      m.tools.some((t) => typeof t !== 'string' || !/^[a-zA-Z0-9_.-]{1,64}$/.test(t)) ||
      new Set(m.tools).size !== m.tools.length ||
      contributionNames.has(m.id)
    )
      throw invalid()
    contributionNames.add(m.id)
  }
  for (const t of v.tools) {
    if (
      !object(t) ||
      !exact(t, ['name', 'description', 'operation', 'inputSchema', 'mappings']) ||
      typeof t.name !== 'string' ||
      !/^[a-zA-Z0-9_.-]{1,64}$/.test(t.name) ||
      toolNames.has(t.name) ||
      !text(t.description, 1024) ||
      typeof t.operation !== 'string' ||
      !/^[a-zA-Z0-9_.]{1,80}$/.test(t.operation) ||
      !object(t.inputSchema) ||
      t.inputSchema.type !== 'object' ||
      !object(t.inputSchema.properties) ||
      Object.keys(t.inputSchema.properties).length > 64 ||
      !object(t.mappings) ||
      Object.keys(t.mappings).length > 64
    )
      throw invalid()
    const properties = t.inputSchema.properties
    if (
      Object.entries(t.mappings).some(
        ([target, s]) =>
          !/^[a-zA-Z0-9_.-]{1,64}$/.test(target) ||
          typeof s !== 'string' ||
          !/^[a-zA-Z0-9_.-]{1,64}$/.test(s) ||
          !Object.hasOwn(properties, s),
      )
    )
      throw invalid()
    toolNames.add(t.name)
  }
  return { ...v, files, skills } as unknown as PackageInfo
}
export function parseExtension(v: unknown): Extension {
  if (
    !object(v) ||
    !exact(v, [
      'id',
      'ownerId',
      'name',
      'kind',
      'enabled',
      'version',
      'revision',
      'revisionId',
      'package',
      'createdAt',
      'updatedAt',
      'deletedAt',
    ]) ||
    !objectId(v.id) ||
    !objectId(v.ownerId) ||
    !objectId(v.revisionId) ||
    !integer(v.version) ||
    !integer(v.revision) ||
    typeof v.enabled !== 'boolean' ||
    !date(v.createdAt) ||
    !date(v.updatedAt) ||
    !(v.deletedAt === null || date(v.deletedAt))
  )
    throw invalid()
  const p = parsePackage(v.package)
  if (v.name !== p.name || v.kind !== p.kind || v.revision > v.version) throw invalid()
  return { ...v, package: p } as unknown as Extension
}
export function parseRevision(v: unknown): ExtensionRevision {
  if (
    !object(v) ||
    !exact(v, [
      'id',
      'extensionId',
      'ownerId',
      'actorId',
      'requestId',
      'number',
      'package',
      'createdAt',
    ]) ||
    !objectId(v.id) ||
    !objectId(v.extensionId) ||
    !objectId(v.ownerId) ||
    !objectId(v.actorId) ||
    !requestId(v.requestId) ||
    !integer(v.number) ||
    !date(v.createdAt)
  )
    throw invalid()
  return { ...v, package: parsePackage(v.package) } as unknown as ExtensionRevision
}
export function parseConstraints(v: unknown): Constraints {
  if (
    !object(v) ||
    !exact(v, ['version', 'content', 'updatedAt', 'actorId']) ||
    !integer(v.version) ||
    !text(v.content, 65536, true) ||
    new TextEncoder().encode(v.content).length > 65536 ||
    !date(v.updatedAt) ||
    !(v.actorId === null || objectId(v.actorId))
  )
    throw invalid()
  return v as unknown as Constraints
}
export function parsePreview(v: unknown, expected: ExtensionFile): ExtensionPreview {
  if (
    !object(v) ||
    !exact(v, ['path', 'content', 'sha256', 'size']) ||
    !expected.text ||
    !extensionPath(v.path) ||
    v.path !== expected.path ||
    v.sha256 !== expected.sha256 ||
    v.size !== expected.size ||
    typeof v.content !== 'string' ||
    v.content.includes('\0') ||
    new TextEncoder().encode(v.content).length !== v.size ||
    !integer(v.size, 0, 256 * 1024)
  )
    throw invalid()
  return v as unknown as ExtensionPreview
}
export async function verifyPreview(v: ExtensionPreview, file: ExtensionFile) {
  parsePreview(v, file)
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(v.content))
  if (
    Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, '0')).join('') !==
    file.sha256
  )
    throw invalid()
}
function operation(v: unknown): ExtensionOperation {
  if (
    !object(v) ||
    !exact(v, ['requestId', 'kind', 'extensionId', 'version', 'createdAt']) ||
    !requestId(v.requestId) ||
    !date(v.createdAt) ||
    (v.kind !== 'install' && v.kind !== 'revise') ||
    (v.kind === 'install'
      ? v.extensionId !== null || v.version !== null
      : !objectId(v.extensionId) || !integer(v.version))
  )
    throw invalid()
  return v as unknown as ExtensionOperation
}
export function verifyReceipt(
  value: unknown,
  snapshot: unknown,
  pending: ExtensionOperation,
  actor: string,
) {
  const row = parseExtension(value),
    revision = parseRevision(snapshot),
    op = operation(pending)
  if (
    !objectId(actor) ||
    revision.requestId !== op.requestId ||
    revision.actorId !== actor ||
    revision.ownerId !== row.ownerId ||
    revision.id !== row.revisionId ||
    revision.extensionId !== row.id ||
    revision.number !== row.revision ||
    revision.package.archiveSha256 !== row.package.archiveSha256 ||
    (op.kind === 'install'
      ? row.ownerId !== actor || row.version !== 1 || row.revision !== 1
      : row.id !== op.extensionId || row.version !== op.version! + 1)
  )
    throw invalid()
  return row
}
export class ExtensionRecoveryStore {
  private key: string
  constructor(
    private storage: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>,
    endpoint: string,
    actor: string,
  ) {
    if (!objectId(actor)) throw invalid()
    this.key = `tri-admin.extensions.v1:${encodeURIComponent(normalizeEndpoint(endpoint))}:${actor}`
  }
  read(): ExtensionOperation[] {
    const raw = this.storage.getItem(this.key)
    if (raw === null) return []
    if (raw.length > 16384) throw invalid()
    let parsed: unknown
    try {
      parsed = JSON.parse(raw)
    } catch {
      throw invalid()
    }
    if (
      !object(parsed) ||
      !exact(parsed, ['formatVersion', 'operations']) ||
      parsed.formatVersion !== 1 ||
      !Array.isArray(parsed.operations) ||
      parsed.operations.length > 16
    )
      throw invalid()
    const rows = parsed.operations.map(operation)
    if (new Set(rows.map((r) => r.requestId)).size !== rows.length) throw invalid()
    return rows
  }
  put(value: ExtensionOperation) {
    const row = operation(value),
      rows = this.read(),
      old = rows.find((r) => r.requestId === row.requestId)
    if (old && JSON.stringify(old) !== JSON.stringify(row)) throw invalid()
    const next = rows.filter((r) => r.requestId !== row.requestId)
    next.push(row)
    if (next.length > 16) throw invalid()
    this.storage.setItem(this.key, JSON.stringify({ formatVersion: 1, operations: next }))
  }
  remove(id: string) {
    const rows = this.read().filter((r) => r.requestId !== id)
    if (rows.length)
      this.storage.setItem(this.key, JSON.stringify({ formatVersion: 1, operations: rows }))
    else this.storage.removeItem(this.key)
  }
  clear() {
    this.storage.removeItem(this.key)
  }
}
export function encodeBytes(bytes: Uint8Array) {
  if (bytes.length > 4 * 1024 * 1024) throw new Error('文件不能超过 4 MiB')
  let binary = ''
  for (let i = 0; i < bytes.length; i += 32768)
    binary += String.fromCharCode(...bytes.subarray(i, i + 32768))
  return btoa(binary)
}
