import { describe, it, expect } from 'vitest'
import { createHash } from 'node:crypto'
import {
  parseDefinition,
  parseGeneration,
  parseSource,
  verifySource,
  CodegenRecoveryStore,
} from '../src/core/codegen'
const id = '0123456789abcdef01234567',
  uuid = '00000000-0000-4000-8000-000000000001',
  date = '2026-10-26T00:00:00.000Z',
  hash = 'a'.repeat(64)
const schema = {
  resource: 'inventory_items',
  label: '库存',
  fields: [
    { name: 'sku', label: '编号', kind: 'text', required: true, searchable: true, unique: true },
  ],
}
const definition = {
  id,
  ownerId: id,
  requestId: uuid,
  schema,
  version: 1,
  createdAt: date,
  updatedAt: date,
  deletedAt: null,
}
const generation = {
  id,
  ownerId: id,
  requestId: uuid,
  definitionId: id,
  definitionVersion: 1,
  templateVersion: 1,
  schema,
  schemaChecksum: hash,
  sha256: hash,
  size: 100,
  files: [{ path: 'README.md', size: 4, sha256: hash }],
  createdAt: date,
}
describe('code generation public models and recovery', () => {
  it('accepts exact public schema and rejects private fields and coercions', () => {
    expect(parseDefinition(definition).schema.resource).toBe('inventory_items')
    for (const value of [
      { ...definition, fingerprint: 'private' },
      { ...definition, version: '1' },
      { ...definition, schema: { ...schema, fields: [{ ...schema.fields[0], kind: ['text'] }] } },
      { ...definition, schema: { ...schema, fields: [{ ...schema.fields[0], name: 'ownerId' }] } },
    ])
      expect(() => parseDefinition(value)).toThrow()
    expect(parseGeneration(generation).files[0]?.path).toBe('README.md')
    for (const value of [
      { ...generation, archiveBytes: 'private' },
      { ...generation, size: 1024 * 1024 + 1 },
      { ...generation, files: [{ path: '../.env', size: 1, sha256: hash }] },
      { ...generation, files: [generation.files[0], generation.files[0]] },
    ])
      expect(() => parseGeneration(value)).toThrow()
  })
  it('checks preview path, UTF8 length and actual digest against the immutable manifest', async () => {
    const content = '库存源码',
      size = new TextEncoder().encode(content).length,
      sha256 = createHash('sha256').update(content).digest('hex'),
      file = { path: 'schema.json', size, sha256 }
    const source = parseSource({ ...file, content }, file)
    await expect(verifySource(source, file)).resolves.toBeUndefined()
    await expect(verifySource({ ...source, content: '库存假码' }, file)).rejects.toThrow()
    expect(() => parseSource({ ...source, path: 'other' }, file)).toThrow()
  })
  it('persists only operation identities per account and normalized server', () => {
    const values = new Map<string, string>(),
      storage = {
        getItem: (key: string) => values.get(key) ?? null,
        setItem: (key: string, value: string) => {
          values.set(key, value)
        },
        removeItem: (key: string) => {
          values.delete(key)
        },
      }
    const store = new CodegenRecoveryStore(storage, 'http://127.0.0.1:18088', id)
    store.put({
      requestId: uuid,
      kind: 'generation.create',
      definitionId: id,
      definitionVersion: 2,
      createdAt: date,
    })
    expect(
      new CodegenRecoveryStore(storage, 'http://127.0.0.1:18088/api/v1', id).read(),
    ).toHaveLength(1)
    expect(new CodegenRecoveryStore(storage, 'http://localhost:18088', id).read()).toEqual([])
    expect(() => store.put({ ...store.read()[0]!, definitionVersion: 3 })).toThrow()
    expect([...values.values()].join('')).not.toContain('schema')
    store.remove(uuid)
    expect(store.read()).toEqual([])
    store.put({
      requestId: uuid,
      kind: 'generation.create',
      definitionId: id,
      definitionVersion: 2,
      createdAt: date,
    })
    values.set([...values.keys()][0]!, '{"broken":true}')
    expect(() => store.read()).toThrow()
    store.clear()
    expect(store.read()).toEqual([])
  })
})
