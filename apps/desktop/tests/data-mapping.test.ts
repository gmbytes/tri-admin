import { it, expect } from 'vitest'
import {
  parseInspection,
  defaultMapping,
  mappingProblem,
  effectiveMapping,
} from '../src/core/data-mapping'
import type { DataResource } from '../src/core/data-tools'
const resource: DataResource = {
  key: 'parameters',
  label: '参数',
  fields: ['name', 'key', 'group', 'valueType', 'value'],
  requiredFields: ['name', 'key', 'valueType', 'value'],
  importable: true,
  readPermission: 'parameters.read',
  writePermission: 'parameters.write',
}
const expected = { resource: 'parameters', format: 'csv' as const, sha256: 'a'.repeat(64) }
it('binds bounded inspection metadata to the selected source and rejects duplicate headers or raw cells', () => {
  const value = { ...expected, headers: ['名称', '标识', '类型', '值'], rowCount: 2 }
  const parsed = parseInspection(value, expected)
  expect(parsed.headers).toEqual(value.headers)
  expect(Object.isFrozen(parsed.headers)).toBe(true)
  for (const change of [
    { sha256: 'b'.repeat(64) },
    { format: 'xlsx' },
    { resource: 'users' },
    { headers: ['same', 'same'] },
    { rowCount: 2001 },
    { headers: Array(33).fill('x') },
    { cells: [['private']] },
  ])
    expect(() => parseInspection({ ...value, ...change }, expected)).toThrow()
})
it('starts from exact header matches and requires explicit valid unique field assignments', () => {
  const headers = ['name', '标识', '类型', '值'],
    mapping = defaultMapping(resource, headers)
  expect(mapping).toEqual({ name: 'name', key: '', group: '', valueType: '', value: '' })
  expect(mappingProblem(resource, mapping, headers)?.code).toBe('MISSING_REQUIRED')
  const ready = { ...mapping, key: '标识', valueType: '类型', value: '值' }
  expect(mappingProblem(resource, ready, headers)).toBeNull()
  expect(effectiveMapping(resource, ready, headers)).toEqual({
    name: 'name',
    key: '标识',
    valueType: '类型',
    value: '值',
  })
  expect(mappingProblem(resource, { ...ready, key: 'name' }, headers)?.code).toBe(
    'DUPLICATE_HEADER',
  )
  expect(mappingProblem(resource, { ...ready, key: 'not in file' }, headers)?.code).toBe(
    'UNKNOWN_HEADER',
  )
  expect(() => effectiveMapping(resource, { ...ready, password: '值' }, headers)).toThrow()
})
