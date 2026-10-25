import type { DataResource } from './data-tools'
export type FilterOp = 'eq' | 'prefix' | 'gt' | 'gte' | 'lt' | 'lte' | 'in'
export interface DataFilter {
  field: string
  op: FilterOp
  value: unknown
}
export interface DataSort {
  field: string
  direction: 'asc' | 'desc'
}
export interface QueryField {
  field: string
  label: string
  kind: 'text' | 'boolean' | 'integer' | 'timestamp' | 'id' | 'choice'
  operators: FilterOp[]
  choices: string[]
}
export interface QuerySchema {
  fields: QueryField[]
  sortFields: string[]
}
export interface TemplateColumn {
  field: string
  header: string
}
export interface ExportTemplate {
  id: string
  name: string
  code: string
  resource: string
  columns: TemplateColumn[]
  search: string
  filters: DataFilter[]
  sort: DataSort | null
  maxRows: number
  allowImport: boolean
  enabled: boolean
  version: number
  createdAt: string
  updatedAt: string
}
const operators: FilterOp[] = ['eq', 'prefix', 'gt', 'gte', 'lt', 'lte', 'in']
const timestamp = (value: unknown): value is string =>
  typeof value === 'string' &&
  Number.isFinite(Date.parse(value)) &&
  new Date(value).toISOString() === value
function invalid() {
  return new Error('模板配置或查询字段无效')
}
export function parseQuerySchema(value: unknown): QuerySchema {
  const data = value as QuerySchema
  if (
    !data ||
    !Array.isArray(data.fields) ||
    data.fields.length > 32 ||
    !Array.isArray(data.sortFields) ||
    data.sortFields.length > 32 ||
    data.sortFields.some((v) => typeof v !== 'string' || !/^[A-Za-z][A-Za-z0-9]*$/.test(v))
  )
    throw invalid()
  for (const field of data.fields)
    if (
      !field ||
      typeof field.field !== 'string' ||
      !/^[A-Za-z][A-Za-z0-9]*$/.test(field.field) ||
      typeof field.label !== 'string' ||
      !['text', 'boolean', 'integer', 'timestamp', 'id', 'choice'].includes(field.kind) ||
      !Array.isArray(field.operators) ||
      !field.operators.length ||
      field.operators.some((op) => !operators.includes(op)) ||
      !Array.isArray(field.choices) ||
      field.choices.length > 32 ||
      field.choices.some((v) => typeof v !== 'string')
    )
      throw invalid()
  if (new Set(data.fields.map((field) => field.field)).size !== data.fields.length) throw invalid()
  return data
}
function scalar(value: unknown): boolean {
  return typeof value === 'string'
    ? Array.from(value).length <= 255
    : typeof value === 'boolean' || (typeof value === 'number' && Number.isSafeInteger(value))
}
export function parseTemplate(value: unknown, resources: DataResource[]): ExportTemplate {
  const data = value as ExportTemplate,
    resource = resources.find((resource) => resource.key === data?.resource)
  if (
    !data ||
    !resource ||
    typeof data.id !== 'string' ||
    !/^[a-f0-9]{24}$/.test(data.id) ||
    typeof data.name !== 'string' ||
    !data.name.trim() ||
    Array.from(data.name).length > 80 ||
    typeof data.code !== 'string' ||
    !/^[a-z][a-z0-9._-]{0,63}$/.test(data.code) ||
    !Number.isSafeInteger(data.version) ||
    data.version < 1 ||
    !timestamp(data.createdAt) ||
    !timestamp(data.updatedAt) ||
    typeof data.enabled !== 'boolean' ||
    typeof data.allowImport !== 'boolean' ||
    !Number.isInteger(data.maxRows) ||
    data.maxRows < 1 ||
    data.maxRows > 2000 ||
    typeof data.search !== 'string' ||
    Array.from(data.search).length > 100 ||
    !Array.isArray(data.columns) ||
    !data.columns.length ||
    data.columns.length > 32 ||
    !Array.isArray(data.filters) ||
    data.filters.length > 8
  )
    throw invalid()
  for (const column of data.columns)
    if (
      !column ||
      !resource.fields.includes(column.field) ||
      typeof column.header !== 'string' ||
      !column.header ||
      column.header.trim() !== column.header ||
      new TextEncoder().encode(column.header).length > 128 ||
      /[\u0000-\u001f\u007f-\u009f]/u.test(column.header) ||
      /^[=+@-]/.test(column.header)
    )
      throw invalid()
  if (
    new Set(data.columns.map((column) => column.field)).size !== data.columns.length ||
    new Set(data.columns.map((column) => column.header)).size !== data.columns.length ||
    (data.allowImport &&
      (!resource.importable ||
        resource.requiredFields.some(
          (field) => !data.columns.some((column) => column.field === field),
        )))
  )
    throw invalid()
  for (const filter of data.filters)
    if (
      !filter ||
      typeof filter.field !== 'string' ||
      !/^[A-Za-z][A-Za-z0-9]*$/.test(filter.field) ||
      !operators.includes(filter.op) ||
      (filter.op === 'in'
        ? !Array.isArray(filter.value) ||
          !filter.value.length ||
          filter.value.length > 32 ||
          filter.value.some((v) => !scalar(v))
        : !scalar(filter.value))
    )
      throw invalid()
  if (
    data.sort !== null &&
    (!data.sort ||
      typeof data.sort.field !== 'string' ||
      !/^[A-Za-z][A-Za-z0-9]*$/.test(data.sort.field) ||
      !['asc', 'desc'].includes(data.sort.direction))
  )
    throw invalid()
  return data
}
export function validateFilters(filters: DataFilter[], schema: QuerySchema): string {
  if (filters.length > 8) return '最多设置 8 个筛选条件'
  for (const filter of filters) {
    const field = schema.fields.find((field) => field.field === filter.field)
    if (!field || !field.operators.includes(filter.op)) return '请选择资源允许的筛选字段和操作'
    const values =
      filter.op === 'in' ? (Array.isArray(filter.value) ? filter.value : []) : [filter.value]
    if (!values.length || values.length > 32) return `${field.label} 需要 1–32 个值`
    for (const value of values) {
      const valid =
        field.kind === 'boolean'
          ? typeof value === 'boolean'
          : field.kind === 'integer'
            ? typeof value === 'number' &&
              Number.isSafeInteger(value) &&
              (field.field !== 'sort' || (value >= -2147483648 && value <= 2147483647))
            : field.kind === 'timestamp'
              ? typeof value === 'string' &&
                /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/i.test(
                  value,
                ) &&
                Number.isFinite(Date.parse(value))
              : field.kind === 'id'
                ? typeof value === 'string' && /^[a-f0-9]{24}$/i.test(value)
                : field.kind === 'choice'
                  ? typeof value === 'string' && field.choices.includes(value)
                  : typeof value === 'string' &&
                    Array.from(value).length <= 255 &&
                    (filter.op !== 'prefix' || !value.includes('\0'))
      if (!valid) return `${field.label} 的筛选值类型无效`
    }
  }
  return ''
}
export function templateMapping(
  template: ExportTemplate,
  headers: readonly string[],
): Record<string, string> {
  return Object.fromEntries(
    template.columns.map((column) => [
      column.field,
      headers.includes(column.header) ? column.header : '',
    ]),
  )
}
