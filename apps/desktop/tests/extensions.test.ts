import { describe, it, expect } from 'vitest'
import { createHash } from 'node:crypto'
import {
  parseExtension,
  parseRevision,
  parseConstraints,
  parsePreview,
  verifyPreview,
  ExtensionRecoveryStore,
  verifyReceipt,
} from '../src/core/extensions'
const id = '0123456789abcdef01234567',
  revisionId = '1123456789abcdef01234567',
  uuid = '00000000-0000-4000-8000-000000000001',
  date = '2026-10-27T00:00:00.000Z',
  hash = 'a'.repeat(64)
const info = {
  kind: 'skill',
  name: 'record-helper',
  version: null,
  description: 'Records',
  skills: [
    {
      meta: {
        name: 'record-helper',
        description: 'Records',
        license: null,
        compatibility: null,
        metadata: {},
        allowedTools: null,
      },
      entryPath: 'SKILL.md',
      sha256: hash,
    },
  ],
  mcp: [],
  tools: [],
  files: [{ path: 'SKILL.md', size: 10, mode: 420, sha256: hash, text: true }],
  archiveSha256: hash,
  treeSha256: hash,
  archiveSize: 100,
  expandedSize: 10,
}
const extension = {
  id,
  ownerId: id,
  name: 'record-helper',
  kind: 'skill',
  enabled: false,
  version: 1,
  revision: 1,
  revisionId,
  package: info,
  createdAt: date,
  updatedAt: date,
  deletedAt: null,
}
const revision = {
  id: revisionId,
  extensionId: id,
  ownerId: id,
  actorId: id,
  requestId: uuid,
  number: 1,
  package: info,
  createdAt: date,
}
describe('extension public models and recovery', () => {
  it('rejects malformed private and ambiguous package metadata', () => {
    expect(parseExtension(extension).package.files[0]?.path).toBe('SKILL.md')
    expect(parseRevision(revision).requestId).toBe(uuid)
    expect(() => parseRevision({ ...revision, package: { ...info, kind: ['skill'] } })).toThrow()
    for (const value of [
      { ...extension, fingerprint: 'private' },
      { ...extension, version: '1' },
      { ...extension, package: { ...info, files: [...info.files, ...info.files] } },
      { ...extension, package: { ...info, archiveSize: 4 * 1024 * 1024 + 1 } },
      { ...extension, package: { ...info, files: [{ ...info.files[0], path: '../escape' }] } },
      { ...extension, name: 'different' },
    ])
      expect(() => parseExtension(value)).toThrow()
    expect(
      parseConstraints({ version: 1, content: '', updatedAt: date, actorId: null }).content,
    ).toBe('')
    expect(() =>
      parseConstraints({ version: 1, content: 'x'.repeat(65537), updatedAt: date, actorId: null }),
    ).toThrow()
  })
  it('validates Unicode file previews against the selected immutable manifest', async () => {
    const content = '资源模板',
      size = new TextEncoder().encode(content).length,
      sha256 = createHash('sha256').update(content).digest('hex'),
      file = { path: 'templates/记录.txt', size, mode: 420, sha256, text: true }
    const preview = parsePreview({ path: file.path, size, sha256, content }, file)
    await expect(verifyPreview(preview, file)).resolves.toBeUndefined()
    await expect(verifyPreview({ ...preview, content: '资源伪造' }, file)).rejects.toThrow()
    expect(() => parsePreview({ ...preview, path: 'other.txt' }, file)).toThrow()
  })
  it('stores only bounded operation identities per account and server', () => {
    const data = new Map<string, string>(),
      storage = {
        getItem: (key: string) => data.get(key) ?? null,
        setItem: (key: string, value: string) => {
          data.set(key, value)
        },
        removeItem: (key: string) => {
          data.delete(key)
        },
      }
    const store = new ExtensionRecoveryStore(storage, 'http://localhost:18088/api/v1', id),
      operation = {
        requestId: uuid,
        kind: 'install' as const,
        extensionId: null,
        version: null,
        createdAt: date,
      }
    store.put(operation)
    expect(store.read()).toEqual([operation])
    expect([...data.values()].join('')).not.toContain('Records')
    expect(() => store.put({ ...operation, payload: 'secret' } as never)).toThrow()
    expect(new ExtensionRecoveryStore(storage, 'http://localhost:18089/api/v1', id).read()).toEqual(
      [],
    )
    expect(
      new ExtensionRecoveryStore(storage, 'http://localhost:18088/api/v1', revisionId).read(),
    ).toEqual([])
    verifyReceipt(extension, revision, operation, id)
    expect(() =>
      verifyReceipt(
        extension,
        { ...revision, requestId: '00000000-0000-4000-8000-000000000002' },
        operation,
        id,
      ),
    ).toThrow()
    expect(() =>
      verifyReceipt(
        extension,
        revision,
        { ...operation, kind: 'revise', extensionId: id, version: 1 },
        id,
      ),
    ).toThrow()
    const key = [...data.keys()][0]!
    store.remove(uuid)
    expect(store.read()).toEqual([])
    data.set(key, 'corrupt')
    expect(() => store.read()).toThrow()
  })
})
