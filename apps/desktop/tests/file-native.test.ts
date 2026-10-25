import { createHash } from 'node:crypto'
import { describe, it, expect } from 'vitest'
import { NativeFileAccess, type NativeFileBridge } from '../src/core/file-native'
const id = '414f732f-b0d1-467c-bfd4-adb7f0fce886'
const data = new Uint8Array([1, 2, 3]),
  spec = { size: 3, sha256: createHash('sha256').update(data).digest('hex') }
const info = { name: 'source.bin', ...spec }
type Call = { name: string; args?: unknown; options?: unknown }
function fixture(
  handler: (name: string, args: unknown, options: unknown) => unknown | Promise<unknown>,
) {
  const calls: Call[] = []
  const bridge: NativeFileBridge = {
    channel: () => ({ channel: 'fixture' }),
    invoke: async <T>(
      name: string,
      args?: Record<string, unknown> | Uint8Array,
      options?: { headers?: Record<string, string> },
    ) => {
      calls.push({ name, args, options })
      return (await handler(name, args, options)) as T
    },
  }
  return { access: new NativeFileAccess(bridge), calls }
}
function standard(name: string) {
  if (name === 'file_reserve') return id
  if (name === 'file_select_source') return info
  if (name === 'file_select_download') return true
  if (name === 'file_read_chunk') return data.slice().buffer
  if (name === 'file_complete') return { ...spec, cleanupPending: false }
}
describe('native file access adapter', () => {
  it('batches bounded blocks into ordered one MiB IPC writes and flushes the tail', async () => {
    const bytes = new Uint8Array(2 * 1024 * 1024 + 17).fill(39),
      expected = { size: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') }
    const { access, calls } = fixture((name) =>
      name === 'file_complete' ? { ...expected, cleanupPending: false } : standard(name),
    )
    const sink = await access.chooseTarget('batch.bin', expected)
    for (let offset = 0; offset < bytes.length; offset += 64 * 1024)
      await sink!.write(bytes.subarray(offset, offset + 64 * 1024))
    expect(calls.filter((call) => call.name === 'file_write')).toHaveLength(2)
    await sink!.complete(expected)
    const blocks = calls
      .filter((call) => call.name === 'file_write')
      .map((call) => call.args as Uint8Array)
    expect(blocks.map((block) => block.length)).toEqual([1024 * 1024, 1024 * 1024, 17])
    expect(Buffer.concat(blocks).equals(Buffer.from(bytes))).toBe(true)
  })
  it('discards buffered target data on cancellation instead of flushing it', async () => {
    const expected = { size: 64 * 1024, sha256: '0'.repeat(64) },
      { access, calls } = fixture(standard),
      sink = await access.chooseTarget('cancel.bin', expected)
    await sink!.write(new Uint8Array(64 * 1024))
    await sink!.abort()
    expect(calls.some((call) => call.name === 'file_write')).toBe(false)
    expect(calls.some((call) => call.name === 'file_complete')).toBe(false)
  })
  it('uses opaque handles for source chunks and releases them on close', async () => {
    const { access, calls } = fixture(standard)
    const source = await access.openSource()
    expect(source).toMatchObject(info)
    const chunk = await source!.readChunk(0)
    expect(Buffer.from(await chunk.bytes.arrayBuffer()).equals(Buffer.from(data))).toBe(true)
    expect(chunk.sha256).toBe(spec.sha256)
    await source!.close()
    await source!.close()
    await expect(source!.readChunk(0)).rejects.toMatchObject({ code: 'REQUEST_CANCELLED' })
    expect(calls.filter((call) => call.name === 'file_cancel')).toHaveLength(1)
    expect(calls.find((call) => call.name === 'file_read_chunk')?.args).toEqual({
      handle: id,
      index: 0,
    })
    expect(JSON.stringify(calls)).not.toContain('path')
  })
  it('writes raw bounded bytes and validates the saved receipt', async () => {
    const { access, calls } = fixture(standard),
      sink = await access.chooseTarget('saved.bin', spec)
    await sink!.write(data)
    expect(await sink!.complete(spec)).toEqual({ kind: 'native', ...spec, cleanupPending: false })
    const call = calls.find((call) => call.name === 'file_write')!
    expect(call.args).toBeInstanceOf(Uint8Array)
    expect(call.options).toEqual({ headers: { 'x-tri-file-handle': id } })
    expect(calls.find((call) => call.name === 'file_select_download')?.args).toEqual({
      handle: id,
      name: 'saved.bin',
      spec,
    })
  })
  it('does not send oversized blocks or accept extra private metadata fields', async () => {
    const { access, calls } = fixture(standard),
      sink = await access.chooseTarget('saved.bin', spec)
    await expect(sink!.write(new Uint8Array(64 * 1024 + 1))).rejects.toMatchObject({
      code: 'FILE_BLOCK_INVALID',
    })
    await sink!.abort()
    expect(calls.some((call) => call.name === 'file_write')).toBe(false)
    const invalid = fixture((name) =>
      name === 'file_select_source' ? { ...info, path: 'C:/private/source.bin' } : standard(name),
    )
    await expect(invalid.access.openSource()).rejects.toMatchObject({
      code: 'FILE_METADATA_INVALID',
    })
    expect(invalid.calls.filter((call) => call.name === 'file_cancel')).toHaveLength(1)
  })
  it('retires a reservation that arrives after the session was invalidated', async () => {
    let release!: (id: string) => void, start!: () => void
    const started = new Promise<void>((resolve) => {
        start = resolve
      }),
      reserved = new Promise<string>((resolve) => {
        release = resolve
      })
    const { access, calls } = fixture((name) => {
      if (name === 'file_reserve') {
        start()
        return reserved
      }
      return standard(name)
    })
    const pending = access.openSource().catch((error) => error)
    await started
    await access.invalidate()
    release(id)
    expect(await pending).toMatchObject({ code: 'SESSION_CHANGED' })
    expect(calls.some((call) => call.name === 'file_select_source')).toBe(false)
    expect(calls.filter((call) => call.name === 'file_cancel')).toHaveLength(1)
  })
  it('discards a source selected after its session has changed', async () => {
    let release!: (value: typeof info) => void, start!: () => void
    const started = new Promise<void>((resolve) => {
        start = resolve
      }),
      selected = new Promise<typeof info>((resolve) => {
        release = resolve
      })
    const { access } = fixture((name) => {
      if (name === 'file_select_source') {
        start()
        return selected
      }
      return standard(name)
    })
    const pending = access.openSource().catch((error) => error)
    await started
    await access.invalidate()
    release(info)
    expect(await pending).toMatchObject({ code: 'SESSION_CHANGED' })
  })
  it('requests cancellation during source preparation and drops its eventual result', async () => {
    let release!: (value: typeof info) => void, start!: () => void
    const started = new Promise<void>((resolve) => {
        start = resolve
      }),
      selected = new Promise<typeof info>((resolve) => {
        release = resolve
      })
    const { access, calls } = fixture((name) => {
      if (name === 'file_select_source') {
        start()
        return selected
      }
      return standard(name)
    })
    const controller = new AbortController(),
      pending = access.openSource({ signal: controller.signal }).catch((error) => error)
    await started
    controller.abort()
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(calls.some((call) => call.name === 'file_cancel')).toBe(true)
    release(info)
    expect(await pending).toMatchObject({ code: 'REQUEST_CANCELLED' })
  })
  it('prevents target commit after cancellation or a session change', async () => {
    const { access, calls } = fixture(standard),
      sink = await access.chooseTarget('saved.bin', spec)
    await sink!.write(data)
    await access.invalidate()
    await expect(sink!.complete(spec)).rejects.toMatchObject({ code: 'SESSION_CHANGED' })
    expect(calls.some((call) => call.name === 'file_complete')).toBe(false)
  })
  it('waits for native cancellation before reserving files for the next session', async () => {
    let release!: () => void
    const gate = new Promise<void>((resolve) => {
        release = resolve
      }),
      { access, calls } = fixture((name) => (name === 'file_cancel_all' ? gate : standard(name)))
    const reset = access.invalidate(),
      pending = access.openSource()
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(calls.some((call) => call.name === 'file_reserve')).toBe(false)
    release()
    await reset
    const source = await pending
    expect(source).toMatchObject(info)
    await source!.close()
  })
})
