import type { DataFormat, DataResource } from './data-tools'
export interface DataInspection {
  readonly resource: string
  readonly format: DataFormat
  readonly sha256: string
  readonly headers: readonly string[]
  readonly rowCount: number
}
export function parseInspection(
  value: unknown,
  expected: { resource: string; format: DataFormat; sha256: string },
): DataInspection {
  const data = value as DataInspection
  if (
    !data ||
    typeof data !== 'object' ||
    Object.keys(data).sort().join(',') !== 'format,headers,resource,rowCount,sha256' ||
    data.resource !== expected.resource ||
    data.format !== expected.format ||
    typeof data.sha256 !== 'string' ||
    !/^[a-f0-9]{64}$/.test(data.sha256) ||
    data.sha256 !== expected.sha256 ||
    !Number.isSafeInteger(data.rowCount) ||
    data.rowCount < 0 ||
    data.rowCount > 2000 ||
    !Array.isArray(data.headers) ||
    data.headers.length < 1 ||
    data.headers.length > 32 ||
    data.headers.some(
      (header) =>
        typeof header !== 'string' ||
        !header.trim() ||
        new TextEncoder().encode(header).length > 128 ||
        /[\u0000-\u001f\u007f-\u009f]/u.test(header),
    ) ||
    new Set(data.headers).size !== data.headers.length
  )
    throw new Error('文件表头或内容摘要校验失败，请重新选择文件')
  return Object.freeze({
    resource: data.resource,
    format: data.format,
    sha256: data.sha256,
    headers: Object.freeze([...data.headers]),
    rowCount: data.rowCount,
  })
}
export interface MappingIssue {
  code: 'UNKNOWN_FIELD' | 'UNKNOWN_HEADER' | 'MISSING_REQUIRED' | 'DUPLICATE_HEADER'
  fields: string[]
}
export function defaultMapping(
  resource: DataResource,
  headers: readonly string[],
): Record<string, string> {
  return Object.fromEntries(
    resource.fields.map((field) => [field, headers.includes(field) ? field : '']),
  )
}
export function mappingProblem(
  resource: DataResource,
  mapping: Record<string, string>,
  headers: readonly string[],
): MappingIssue | null {
  const unknown = Object.keys(mapping).filter((field) => !resource.fields.includes(field))
  if (unknown.length || resource.requiredFields.some((field) => !resource.fields.includes(field)))
    return { code: 'UNKNOWN_FIELD', fields: unknown }
  const missing = resource.requiredFields.filter((field) => !mapping[field])
  if (missing.length) return { code: 'MISSING_REQUIRED', fields: missing }
  const used = new Map<string, string>()
  for (const field of resource.fields) {
    const header = mapping[field]
    if (header === undefined || header === '') continue
    if (typeof header !== 'string' || !headers.includes(header))
      return { code: 'UNKNOWN_HEADER', fields: [field] }
    const previous = used.get(header)
    if (previous) return { code: 'DUPLICATE_HEADER', fields: [previous, field] }
    used.set(header, field)
  }
  return null
}
export function effectiveMapping(
  resource: DataResource,
  mapping: Record<string, string>,
  headers: readonly string[],
): Record<string, string> {
  if (mappingProblem(resource, mapping, headers))
    throw new Error('请为必填字段选择不同的有效来源列')
  return Object.fromEntries(
    resource.fields.filter((field) => !!mapping[field]).map((field) => [field, mapping[field]]),
  )
}
