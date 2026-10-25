import { describe, it, expect } from 'vitest'
import { ApiError } from '../src/core/client'
import { UploadController, type UploadTransport } from '../src/core/upload-controller'
import { UploadJournal, type UploadJournalStorage } from '../src/core/upload-journal'
import type { Envelope } from '../src/core/types'
import type { UploadRecord, UploadRequest } from '../src/core/file-models'
import type { UploadSource } from '../src/core/files'
const scope = { endpoint: 'http://127.0.0.1:18088/api/v1', actorId: '0123456789abcdef01234567' },
  id = '1123456789abcdef01234567',
  profile = '2123456789abcdef01234567'
const request: UploadRequest = {
  requestId: '00000000-0000-4000-8000-000000000001',
  name: 'upload.bin',
  mediaType: 'application/octet-stream',
  size: 4 * 1024 * 1024 + 3,
  sha256: 'a'.repeat(64),
}
function store(): UploadJournalStorage {
  const values = new Map<string, string>()
  return {
    get length() {
      return values.size
    },
    key: (n) => [...values.keys()][n] ?? null,
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => {
      values.set(key, value)
    },
    removeItem: (key) => {
      values.delete(key)
    },
  }
}
function harness(input = request) {
  const storage = store(),
    journal = new UploadJournal(storage, scope)
  journal.create(input)
  let created = false,
    creationLost = false,
    lostAck = -1,
    acceptLost = true,
    live = true
  const calls: string[] = [],
    puts: number[] = [],
    creates: unknown[] = []
  const row: UploadRecord = {
    id,
    ownerId: scope.actorId,
    requestId: input.requestId,
    name: input.name,
    mediaType: input.mediaType,
    size: input.size,
    sha256: input.sha256,
    categoryId: input.categoryId ?? null,
    storageProfileId: input.storageProfileId ?? profile,
    state: 'receiving',
    chunkSize: 4 * 1024 * 1024,
    totalChunks: Math.ceil(input.size / (4 * 1024 * 1024)),
    receivedCount: 0,
    receivedBytes: 0,
    receivedIndices: [],
    cleanupPending: false,
    fileId: null,
    lastError: null,
    completedAt: null,
    version: 1,
    createdAt: '2026-10-25T00:00:00.000Z',
    updatedAt: '2026-10-25T00:00:00.000Z',
  }
  const accept = (index: number) => {
    if (!row.receivedIndices!.includes(index)) {
      row.receivedIndices = [...row.receivedIndices!, index].sort()
      row.receivedCount++
      row.receivedBytes += Math.min(row.chunkSize, row.size - index * row.chunkSize)
      row.version++
    }
  }
  const api: UploadTransport = {
    request: async <T>(
      path: string,
      options: { method?: string; body?: unknown; signal?: AbortSignal } = {},
    ) => {
      calls.push((options.method ?? 'GET') + ' ' + path)
      if (path === '/uploads' && options.method === 'POST') {
        creates.push(options.body)
        created = true
        if (creationLost) {
          creationLost = false
          throw new ApiError('NETWORK_ERROR', 'lost creation response')
        }
      } else if (!created) throw new Error('session was not created')
      if (path.endsWith('/complete')) {
        row.state = 'completed'
        row.fileId = id
        row.completedAt = row.updatedAt
        row.version++
      }
      if (path.endsWith('/cancel')) {
        row.state = 'cancelled'
        row.version++
      }
      if (path.endsWith('/retry')) {
        row.state = 'unknown'
        row.version++
      }
      if (path.endsWith('/recover'))
        return { data: { recovered: 0, retired: 0, unknown: 0 } } as Envelope<T>
      if (path.endsWith('/cleanup'))
        return { data: { uploadId: id, processed: 0, pending: 0, complete: true } } as Envelope<T>
      return { data: JSON.parse(JSON.stringify(row)) } as Envelope<T>
    },
    putChunk: async <T>(_upload: string, index: number, _bytes: Blob, sha256: string) => {
      puts.push(index)
      if (index === lostAck) {
        lostAck = -1
        if (acceptLost) accept(index)
        throw new ApiError('NETWORK_ERROR', 'lost chunk response')
      }
      accept(index)
      return {
        data: {
          uploadId: id,
          index,
          size: Math.min(row.chunkSize, row.size - index * row.chunkSize),
          sha256,
        },
      } as Envelope<T>
    },
  }
  const reads: number[] = []
  let closed = 0
  const source = (): UploadSource => ({
    name: 'selected.bin',
    mediaType: input.mediaType,
    size: input.size,
    sha256: input.sha256,
    readChunk: async (index) => {
      reads.push(index)
      return {
        bytes: new Blob([
          new Uint8Array(Math.min(row.chunkSize, row.size - index * row.chunkSize)),
        ]),
        sha256: 'b'.repeat(64),
      }
    },
    close: async () => {
      closed++
    },
  })
  const controller = () => new UploadController(api, scope, { journal, isCurrent: () => live })
  return {
    api,
    journal,
    row,
    controller,
    source,
    calls,
    puts,
    creates,
    reads,
    accept,
    get closed() {
      return closed
    },
    created: () => {
      created = true
    },
    loseCreation: () => {
      creationLost = true
    },
    loseChunk: (index: number, accepted: boolean) => {
      lostAck = index
      acceptLost = accepted
    },
    leave: () => {
      live = false
    },
    storage,
  }
}
describe('durable upload coordinator', () => {
  it('stops before chunks when the creation result cannot be saved locally', async () => {
    const h = harness()
    h.storage.setItem = () => {
      throw Error('quota')
    }
    await expect(h.controller().transfer(request.requestId, h.source())).rejects.toMatchObject({
      code: 'UPLOAD_JOURNAL_UNAVAILABLE',
    })
    expect(h.creates).toHaveLength(1)
    expect(h.puts).toEqual([])
    expect(h.journal.get(request.requestId).uploadId).toBeNull()
    expect(h.closed).toBe(1)
  })
  it('rejects a response owned by another actor or with inconsistent receipts', async () => {
    const h = harness()
    h.row.ownerId = '3123456789abcdef01234567'
    await expect(h.controller().transfer(request.requestId, h.source())).rejects.toMatchObject({
      code: 'UPLOAD_RESPONSE_INVALID',
    })
    expect(h.puts).toEqual([])
    const bad = harness()
    bad.created()
    bad.journal.bind(request.requestId, id)
    bad.row.receivedIndices = [99]
    bad.row.receivedCount = 1
    bad.row.receivedBytes = 3
    await expect(bad.controller().transfer(request.requestId, bad.source())).rejects.toMatchObject({
      code: 'UPLOAD_RESPONSE_INVALID',
    })
    expect(bad.puts).toEqual([])
  })
  it('pauses promptly while a source read is pending and releases the source', async () => {
    const h = harness(),
      controller = h.controller()
    let start!: () => void
    const started = new Promise<void>((resolve) => {
        start = resolve
      }),
      source = {
        ...h.source(),
        readChunk: async () => {
          start()
          return await new Promise<never>(() => {})
        },
      }
    const pending = controller.transfer(request.requestId, source).catch((error) => error)
    await started
    controller.stop(request.requestId)
    expect(await pending).toMatchObject({ code: 'REQUEST_CANCELLED' })
    expect(h.closed).toBe(1)
    expect(controller.isActive(request.requestId)).toBe(false)
  })
  it('replays the exact creation UUID after a lost response and does not change default binding', async () => {
    const h = harness()
    h.loseCreation()
    await expect(h.controller().transfer(request.requestId, h.source())).rejects.toMatchObject({
      code: 'NETWORK_ERROR',
    })
    expect(h.journal.get(request.requestId).uploadId).toBeNull()
    await h.controller().transfer(request.requestId, h.source())
    expect(h.creates).toEqual([request, request])
    expect(h.creates.every((value) => !('storageProfileId' in (value as object)))).toBe(true)
    expect(h.puts).toEqual([0, 1])
    expect(h.closed).toBe(2)
  })
  it('reads durable receipts and skips already accepted chunks after a reload', async () => {
    const h = harness()
    h.created()
    h.accept(1)
    h.journal.bind(request.requestId, id)
    const result = await h.controller().transfer(request.requestId, h.source())
    expect(result.state).toBe('completed')
    expect(h.reads).toEqual([0])
    expect(h.puts).toEqual([0])
  })
  it('queries after a lost chunk acknowledgement and never sends the same accepted chunk again', async () => {
    const h = harness()
    h.loseChunk(0, true)
    await h.controller().transfer(request.requestId, h.source())
    expect(h.puts).toEqual([0, 1])
    expect(h.calls.filter((value) => value === `GET /uploads/${id}`).length).toBeGreaterThan(0)
  })
  it('stops an unacknowledged missing chunk and resumes only on a new user action', async () => {
    const h = harness()
    h.loseChunk(0, false)
    await expect(h.controller().transfer(request.requestId, h.source())).rejects.toMatchObject({
      code: 'NETWORK_ERROR',
    })
    expect(h.puts).toEqual([0])
    expect(h.calls.some((value) => value.endsWith('/complete'))).toBe(false)
    await h.controller().transfer(request.requestId, h.source())
    expect(h.puts).toEqual([0, 0, 1])
  })
  it('rejects a different source before network activity and always closes it', async () => {
    const h = harness()
    await expect(
      h.controller().transfer(request.requestId, { ...h.source(), sha256: 'c'.repeat(64) }),
    ).rejects.toMatchObject({ code: 'UPLOAD_SOURCE_MISMATCH' })
    expect(h.calls).toEqual([])
    expect(h.closed).toBe(1)
  })
  it('keeps an uncertain action until a query confirms its state and never automatically retries publication', async () => {
    const h = harness()
    h.created()
    h.journal.bind(request.requestId, id)
    h.row.state = 'unknown'
    h.journal.mark(request.requestId, 'retry')
    await h.controller().query(request.requestId)
    expect(h.calls.some((value) => value.endsWith('/retry'))).toBe(false)
    await h.controller().action(request.requestId, 'retry')
    expect(h.calls.filter((value) => value.endsWith('/retry'))).toHaveLength(1)
    expect(h.journal.get(request.requestId).pendingAction).toBeNull()
  })
  it('does not bind or send chunks after its account context changes', async () => {
    const h = harness()
    let release!: (value: Envelope<UploadRecord>) => void, start!: () => void
    const started = new Promise<void>((resolve) => {
        start = resolve
      }),
      gate = new Promise<Envelope<UploadRecord>>((resolve) => {
        release = resolve
      })
    const api: UploadTransport = {
      ...h.api,
      request: async <T>() => {
        start()
        return (await gate) as Envelope<T>
      },
    }
    const controller = new UploadController(api, scope, {
      journal: h.journal,
      isCurrent: () => current,
    })
    let current = true
    const pending = controller.transfer(request.requestId, h.source()).catch((error) => error)
    await started
    current = false
    release({ data: h.row })
    expect(await pending).toMatchObject({ code: 'SESSION_CHANGED' })
    expect(h.puts).toEqual([])
    expect(h.journal.get(request.requestId).uploadId).toBeNull()
    expect(h.closed).toBe(1)
  })
  it('supports empty content without attempting a nonexistent chunk', async () => {
    const input = { ...request, size: 0 },
      h = harness(input)
    const result = await h.controller().transfer(input.requestId, h.source())
    expect(result.state).toBe('completed')
    expect(h.reads).toEqual([])
    expect(h.closed).toBe(1)
  })
})
