import { normalizeEndpoint } from './endpoint'
import { objectId, requestId } from './open-access'
export const fieldKinds = ['text', 'integer', 'number', 'boolean', 'datetime', 'textList'] as const
export type FieldKind = (typeof fieldKinds)[number]
export interface CodeField {
  name: string
  label: string
  kind: FieldKind
  required: boolean
  searchable: boolean
  unique: boolean
}
export interface CodeSchema {
  resource: string
  label: string
  fields: CodeField[]
}
export interface CodeDefinition {
  id: string
  ownerId: string
  requestId: string
  schema: CodeSchema
  version: number
  createdAt: string
  updatedAt: string
  deletedAt: string | null
}
export interface GeneratedFile {
  path: string
  size: number
  sha256: string
}
export interface CodeGeneration {
  id: string
  ownerId: string
  requestId: string
  definitionId: string
  definitionVersion: number
  templateVersion: number
  schema: CodeSchema
  schemaChecksum: string
  sha256: string
  size: number
  files: GeneratedFile[]
  createdAt: string
}
export interface GeneratedSource extends GeneratedFile {
  content: string
}
export interface CodegenOperation {
  requestId: string
  kind: 'definition.create' | 'generation.create'
  definitionId: string | null
  definitionVersion: number | null
  createdAt: string
}
function invalid() {
  return new Error('代码生成响应或恢复记录无效')
}
function object(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === 'object' && !Array.isArray(value)
}
function exact(value: Record<string, unknown>, keys: string[]) {
  return Object.keys(value).sort().join('|') === [...keys].sort().join('|')
}
function integer(value: unknown, min = 1, max = Number.MAX_SAFE_INTEGER): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= min && value <= max
}
function text(value: unknown, max = 80): value is string {
  return (
    typeof value === 'string' &&
    !!value.trim() &&
    [...value].length <= max &&
    !/[\u0000-\u001f\u007f-\u009f]/.test(value)
  )
}
function date(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/.test(value) &&
    Number.isFinite(Date.parse(value)) &&
    new Date(value).toISOString() === value
  )
}
function hash(value: unknown): value is string {
  return typeof value === 'string' && /^[a-f0-9]{64}$/.test(value)
}
function fieldName(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    /^[a-z][a-zA-Z0-9]{0,47}$/.test(value) &&
    ![
      'id',
      'ownerid',
      'version',
      'createdat',
      'updatedat',
      'deletedat',
      'application',
      'password',
      'passwordhash',
      'secret',
      'token',
      'authorization',
      'constructor',
      'prototype',
    ].includes(value.toLowerCase())
  )
}
function path(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    value.length > 0 &&
    value.length <= 256 &&
    /^[a-zA-Z0-9._/-]+$/.test(value) &&
    value.split('/').every((part) => !!part && part !== '.' && part !== '..')
  )
}
export function parseSchema(value: unknown): CodeSchema {
  if (
    !object(value) ||
    !exact(value, ['resource', 'label', 'fields']) ||
    typeof value.resource !== 'string' ||
    !/^[a-z][a-z0-9_]{2,47}$/.test(value.resource) ||
    [
      'users',
      'roles',
      'menus',
      'sessions',
      'admins',
      'applications',
      'api_tokens',
      'schema',
      'system',
    ].includes(value.resource) ||
    !text(value.label) ||
    !Array.isArray(value.fields) ||
    value.fields.length < 1 ||
    value.fields.length > 40
  )
    throw invalid()
  const names = new Set<string>()
  let searches = 0,
    uniques = 0
  for (const field of value.fields) {
    if (
      !object(field) ||
      !exact(field, ['name', 'label', 'kind', 'required', 'searchable', 'unique']) ||
      !fieldName(field.name) ||
      names.has(field.name.toLowerCase()) ||
      !text(field.label) ||
      typeof field.kind !== 'string' ||
      !(fieldKinds as readonly string[]).includes(field.kind) ||
      typeof field.required !== 'boolean' ||
      typeof field.searchable !== 'boolean' ||
      typeof field.unique !== 'boolean' ||
      (field.searchable && field.kind !== 'text') ||
      (field.unique && (field.kind !== 'text' || !field.required))
    )
      throw invalid()
    names.add(field.name.toLowerCase())
    if (field.searchable) searches++
    if (field.unique) uniques++
  }
  if (searches > 3 || uniques > 5) throw invalid()
  return value as unknown as CodeSchema
}
export function parseDefinition(value: unknown): CodeDefinition {
  if (
    !object(value) ||
    !exact(value, [
      'id',
      'ownerId',
      'requestId',
      'schema',
      'version',
      'createdAt',
      'updatedAt',
      'deletedAt',
    ]) ||
    !objectId(value.id) ||
    !objectId(value.ownerId) ||
    !requestId(value.requestId) ||
    !integer(value.version) ||
    !date(value.createdAt) ||
    !date(value.updatedAt) ||
    !(value.deletedAt === null || date(value.deletedAt))
  )
    throw invalid()
  return { ...value, schema: parseSchema(value.schema) } as unknown as CodeDefinition
}
function file(value: unknown): GeneratedFile {
  if (
    !object(value) ||
    !exact(value, ['path', 'size', 'sha256']) ||
    !path(value.path) ||
    !integer(value.size, 0, 512 * 1024) ||
    !hash(value.sha256)
  )
    throw invalid()
  return value as unknown as GeneratedFile
}
export function parseGeneration(value: unknown): CodeGeneration {
  if (
    !object(value) ||
    !exact(value, [
      'id',
      'ownerId',
      'requestId',
      'definitionId',
      'definitionVersion',
      'templateVersion',
      'schema',
      'schemaChecksum',
      'sha256',
      'size',
      'files',
      'createdAt',
    ]) ||
    !objectId(value.id) ||
    !objectId(value.ownerId) ||
    !requestId(value.requestId) ||
    !objectId(value.definitionId) ||
    !integer(value.definitionVersion) ||
    !integer(value.templateVersion, 1, 4294967295) ||
    !hash(value.schemaChecksum) ||
    !hash(value.sha256) ||
    !integer(value.size, 1, 1024 * 1024) ||
    !date(value.createdAt) ||
    !Array.isArray(value.files) ||
    value.files.length < 1 ||
    value.files.length > 64
  )
    throw invalid()
  const files = value.files.map(file)
  if (
    new Set(files.map((file) => file.path)).size !== files.length ||
    files.reduce((sum, file) => sum + file.size, 0) > 2 * 1024 * 1024
  )
    throw invalid()
  return { ...value, schema: parseSchema(value.schema), files } as unknown as CodeGeneration
}
export function parseSource(value: unknown, expected: GeneratedFile): GeneratedSource {
  if (
    !object(value) ||
    !exact(value, ['path', 'content', 'size', 'sha256']) ||
    value.path !== expected.path ||
    value.size !== expected.size ||
    value.sha256 !== expected.sha256 ||
    typeof value.content !== 'string' ||
    new TextEncoder().encode(value.content).length !== expected.size
  )
    throw invalid()
  return value as unknown as GeneratedSource
}
export async function verifySource(value: GeneratedSource, expected: GeneratedFile): Promise<void> {
  const bytes = new TextEncoder().encode(value.content)
  if (
    value.path !== expected.path ||
    value.size !== expected.size ||
    value.sha256 !== expected.sha256 ||
    bytes.byteLength !== expected.size
  )
    throw invalid()
  const actual = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)), (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('')
  if (actual !== expected.sha256) throw new Error('生成文件摘要不匹配')
}
function operation(value: unknown): CodegenOperation {
  if (
    !object(value) ||
    !exact(value, ['requestId', 'kind', 'definitionId', 'definitionVersion', 'createdAt']) ||
    !requestId(value.requestId) ||
    !date(value.createdAt) ||
    typeof value.kind !== 'string' ||
    !['definition.create', 'generation.create'].includes(value.kind)
  )
    throw invalid()
  if (
    value.kind === 'generation.create'
      ? !objectId(value.definitionId) || !integer(value.definitionVersion)
      : value.definitionId !== null || value.definitionVersion !== null
  )
    throw invalid()
  return { ...value } as unknown as CodegenOperation
}
export class CodegenRecoveryStore {
  private key: string
  constructor(
    private storage: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>,
    endpoint: string,
    actor: string,
  ) {
    if (!objectId(actor)) throw invalid()
    this.key = `tri-admin.codegen.v1:${encodeURIComponent(normalizeEndpoint(endpoint))}:${actor}`
  }
  read(): CodegenOperation[] {
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
    if (new Set(rows.map((row) => row.requestId)).size !== rows.length) throw invalid()
    return rows
  }
  put(value: CodegenOperation) {
    const row = operation(value),
      rows = this.read(),
      old = rows.find((old) => old.requestId === row.requestId)
    if (
      old &&
      (old.kind !== row.kind ||
        old.definitionId !== row.definitionId ||
        old.definitionVersion !== row.definitionVersion)
    )
      throw invalid()
    const next = rows.filter((old) => old.requestId !== row.requestId)
    next.push(row)
    if (next.length > 16) throw new Error('最多保留 16 项待确认生成操作')
    this.storage.setItem(this.key, JSON.stringify({ formatVersion: 1, operations: next }))
  }
  remove(request: string) {
    const rows = this.read().filter((row) => row.requestId !== request)
    if (rows.length)
      this.storage.setItem(this.key, JSON.stringify({ formatVersion: 1, operations: rows }))
    else this.storage.removeItem(this.key)
  }
  clear() {
    this.storage.removeItem(this.key)
  }
}
