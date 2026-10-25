import { describe, it, expect } from 'vitest'
import { UploadJournal, type UploadJournalStorage } from '../src/core/upload-journal'
function storage() {
  const values = new Map<string, string>()
  const store: UploadJournalStorage = {
    get length() {
      return values.size
    },
    key: (index) => [...values.keys()][index] ?? null,
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => {
      values.set(key, value)
    },
    removeItem: (key) => {
      values.delete(key)
    },
  }
  return { store, values }
}
const scope = { endpoint: 'http://127.0.0.1:18088', actorId: '0123456789abcdef01234567' }
const request = {
  requestId: '00000000-0000-4000-8000-000000000001',
  name: '资料.bin',
  mediaType: 'application/octet-stream',
  size: 3,
  sha256: 'a'.repeat(64),
}
describe('upload metadata journal', () => {
  it('does not let an older response clear a newer pending action', () => {
    const { store } = storage(),
      journal = new UploadJournal(store, scope)
    journal.create(request)
    const earlier = journal.mark(request.requestId, 'complete').pendingAction!,
      later = journal.mark(request.requestId, 'cancel').pendingAction!
    journal.clearAction(request.requestId, earlier.actionId)
    expect(journal.get(request.requestId).pendingAction).toEqual(later)
    journal.clearAction(request.requestId, later.actionId)
    expect(journal.get(request.requestId).pendingAction).toBeNull()
  })
  it('retains exact creation inputs and normalizes server scope without crossing actors', () => {
    const { store, values } = storage(),
      journal = new UploadJournal(store, scope),
      draft = journal.create(request)
    expect(draft.request).toEqual(request)
    expect(draft.uploadId).toBeNull()
    expect('storageProfileId' in draft.request).toBe(false)
    expect(
      new UploadJournal(store, { ...scope, endpoint: scope.endpoint + '/api/v1/' }).list(),
    ).toEqual([draft])
    expect(
      new UploadJournal(store, { ...scope, actorId: '1123456789abcdef01234567' }).list(),
    ).toEqual([])
    expect(new UploadJournal(store, { ...scope, endpoint: 'https://other.test' }).list()).toEqual(
      [],
    )
    expect([...values.values()].join('')).not.toMatch(/password|token|path|bytes|handle/i)
  })
  it('binds the result once and keeps an uncertain action across reload', () => {
    const { store } = storage(),
      journal = new UploadJournal(store, scope)
    journal.create(request)
    journal.bind(request.requestId, '1123456789abcdef01234567')
    journal.mark(request.requestId, 'complete')
    const restored = new UploadJournal(store, scope)
    expect(restored.get(request.requestId).pendingAction?.kind).toBe('complete')
    expect(() => restored.bind(request.requestId, '2123456789abcdef01234567')).toThrow()
    restored.clearAction(request.requestId)
    expect(restored.get(request.requestId).pendingAction).toBeNull()
    restored.forget(request.requestId)
    expect(restored.list()).toEqual([])
  })
  it('does not rewrite immutable requests or persist extra sensitive fields', () => {
    const { store, values } = storage(),
      journal = new UploadJournal(store, scope)
    journal.create(request)
    expect(() => journal.create({ ...request, name: 'changed.bin' })).toThrow()
    expect(() => journal.create({ ...request, refreshToken: 'secret' } as typeof request)).toThrow()
    expect(() =>
      journal.create({
        ...request,
        requestId: '00000000-0000-4000-8000-000000000002',
        path: 'C:/private/file.bin',
      } as typeof request),
    ).toThrow()
    expect([...values.values()].join('')).not.toContain('secret')
    expect(journal.list()).toHaveLength(1)
  })
  it('rejects corrupt records and unavailable storage before allowing a new upload', () => {
    const { store } = storage(),
      journal = new UploadJournal(store, scope)
    journal.create(request)
    store.setItem(store.key(0)!, '{"schemaVersion":0}')
    expect(() => journal.list()).toThrow()
    const broken = {
      ...store,
      get length() {
        return 0
      },
      setItem: () => {
        throw Error('private storage diagnostic')
      },
    }
    expect(() => new UploadJournal(broken, scope).create(request)).toThrow(/续传/)
  })
  it('limits local outstanding records while allowing existing bindings to be updated', () => {
    const { store } = storage(),
      journal = new UploadJournal(store, scope)
    for (let n = 0; n < 32; n++)
      journal.create({
        ...request,
        requestId: `00000000-0000-4000-8000-${String(n).padStart(12, '0')}`,
      })
    expect(() =>
      journal.create({ ...request, requestId: 'ffffffff-ffff-4fff-8fff-ffffffffffff' }),
    ).toThrow(/32/)
    journal.bind(request.requestId, '1123456789abcdef01234567')
    expect(journal.list()).toHaveLength(32)
  })
  it('rejects invalid names, types and scope while returning immutable snapshots', () => {
    const { store } = storage(),
      journal = new UploadJournal(store, scope)
    for (const value of [
      { ...request, name: '../file' },
      { ...request, name: 'CON.txt' },
      { ...request, name: '中'.repeat(90) },
      { ...request, mediaType: 'image/*' },
      { ...request, size: 5 * 1024 * 1024 * 1024 + 1 },
    ])
      expect(() => journal.create(value)).toThrow()
    expect(
      () => new UploadJournal(store, { ...scope, endpoint: 'https://user:private@example.test' }),
    ).toThrow()
    const draft = journal.create(request)
    expect(Object.isFrozen(draft.request)).toBe(true)
  })
})
