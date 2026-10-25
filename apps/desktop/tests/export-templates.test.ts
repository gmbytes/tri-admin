import { it, expect } from 'vitest'
import {
  parseTemplate,
  parseQuerySchema,
  validateFilters,
  templateMapping,
} from '../src/core/export-templates'
const resource = {
  key: 'parameters',
  label: '参数',
  fields: ['name', 'key', 'valueType', 'value'],
  requiredFields: ['name', 'key', 'valueType', 'value'],
  importable: true,
  readPermission: 'parameters.read',
  writePermission: 'parameters.write',
}
const base = {
  id: '1'.repeat(24),
  name: '模板',
  code: 'template.one',
  resource: 'parameters',
  columns: [
    { field: 'name', header: '名称' },
    { field: 'key', header: '标识' },
    { field: 'valueType', header: '类型' },
    { field: 'value', header: '数据' },
  ],
  search: '',
  filters: [],
  sort: null,
  maxRows: 2000,
  allowImport: true,
  enabled: true,
  version: 1,
  createdAt: new Date(0).toISOString(),
  updatedAt: new Date(0).toISOString(),
}
it('keeps ordered columns and matches stored headings without accepting secret fields', () => {
  const template = parseTemplate(base, [resource])
  expect(template.columns[0].header).toBe('名称')
  expect(templateMapping(template, ['标识', '名称', '类型', '数据'])).toEqual({
    name: '名称',
    key: '标识',
    valueType: '类型',
    value: '数据',
  })
  for (const change of [
    { columns: [{ field: 'passwordHash', header: '秘密' }] },
    { columns: [{ field: 'name', header: '=formula' }] },
    { version: 0 },
    { maxRows: 0 },
  ])
    expect(() => parseTemplate({ ...base, ...change }, [resource])).toThrow()
})
it('validates typed filter values against the supplied resource query schema', () => {
  const schema = parseQuerySchema({
    fields: [
      {
        field: 'group',
        label: '分组',
        kind: 'text',
        operators: ['eq', 'prefix', 'in'],
        choices: [],
      },
      { field: 'enabled', label: '启用', kind: 'boolean', operators: ['eq'], choices: [] },
    ],
    sortFields: ['key'],
  })
  expect(validateFilters([{ field: 'group', op: 'eq', value: 'red' }], schema)).toBe('')
  expect(validateFilters([{ field: 'group', op: 'eq', value: { $ne: null } }], schema)).not.toBe('')
  expect(validateFilters([{ field: 'enabled', op: 'eq', value: 'true' }], schema)).not.toBe('')
  expect(validateFilters([{ field: 'group', op: 'gte', value: 'red' }], schema)).not.toBe('')
})
it('rejects locale date strings that the RFC3339 server contract cannot accept', () => {
  const schema = parseQuerySchema({
    fields: [
      { field: 'createdAt', label: '创建时间', kind: 'timestamp', operators: ['gte'], choices: [] },
    ],
    sortFields: ['createdAt'],
  })
  expect(
    validateFilters([{ field: 'createdAt', op: 'gte', value: '2026/10/26' }], schema),
  ).not.toBe('')
  expect(
    validateFilters([{ field: 'createdAt', op: 'gte', value: '2026-10-26T00:00:00.000Z' }], schema),
  ).toBe('')
})
