import { createHash } from 'node:crypto'
import { describe, expect, it, vi } from 'vitest'
import {
  BROWSER_FILE_LIMIT,
  createBrowserFileSink,
  hashBrowserFile,
  readFileContent,
  type FileContentSpec,
  type FileSink,
} from '../src/core/files'

const digest = (data: Uint8Array) => createHash('sha256').update(data).digest('hex')
function response(data: Uint8Array, spec: FileContentSpec, headers: Record<string, string> = {}) {
  return new Response(new Blob([new Uint8Array(data)]), {
    headers: {
      'content-type': 'application/octet-stream',
      'content-disposition': 'attachment; filename="file.bin"',
      'content-length': String(spec.size),
      etag: `"${spec.sha256}"`,
      ...headers,
    },
  })
}

describe('streamed file content', () => {
  it('captures immutable metadata before asynchronous source and target work', async () => {
    const data = new Uint8Array([1, 2, 3]),
      original = { size: 3, sha256: digest(data) }
    const mutable = { ...original },
      target = createBrowserFileSink(mutable)
    mutable.sha256 = '0'.repeat(64)
    const blob = await readFileContent(response(data, original), original, target)
    expect(blob.size).toBe(3)
    const expected = { ...original }
    let captured: FileContentSpec | undefined
    await readFileContent(response(data, original), expected, {
      write: async () => {
        expected.size = 0
      },
      complete: async (value) => {
        captured = value
      },
      abort: async () => {},
    })
    expect(captured).toEqual(original)
  })

  it('streams a source larger than the browser buffer limit into a bounded target', async () => {
    const size = BROWSER_FILE_LIMIT + 17,
      block = new Uint8Array(64 * 1024).fill(37)
    const expected = createHash('sha256')
    let remaining = size
    while (remaining) {
      const length = Math.min(remaining, block.length)
      expected.update(block.subarray(0, length))
      remaining -= length
    }
    const spec = { size, sha256: expected.digest('hex') }
    let offset = 0,
      written = 0,
      max = 0
    const body = new ReadableStream<Uint8Array>({
      pull(controller) {
        if (offset === size) {
          controller.close()
          return
        }
        const length = Math.min(block.length, size - offset)
        controller.enqueue(block.slice(0, length))
        offset += length
      },
    })
    const hash = createHash('sha256'),
      headers = response(new Uint8Array(), spec).headers
    await readFileContent(new Response(body, { headers }), spec, {
      write: async (bytes) => {
        max = Math.max(max, bytes.length)
        written += bytes.length
        hash.update(bytes)
      },
      complete: async (value) => {
        expect(written).toBe(value.size)
        expect(hash.digest('hex')).toBe(value.sha256)
      },
      abort: async () => {},
    })
    expect(written).toBe(size)
    expect(max).toBeLessThanOrEqual(64 * 1024)
  })
  it('writes bounded blocks and exposes a browser blob only after the complete hash matches', async () => {
    const data = new Uint8Array(2 * 1024 * 1024 + 17).fill(91)
    const spec = { size: data.length, sha256: digest(data) }
    const target = createBrowserFileSink(spec)
    const sizes: number[] = [],
      progress: number[] = []
    const sink: FileSink<Blob> = {
      ...target,
      write: async (bytes) => {
        sizes.push(bytes.length)
        await target.write(bytes)
      },
    }
    const blob = await readFileContent(response(data, spec), spec, sink, {
      onProgress: (bytes) => progress.push(bytes),
    })
    expect(Math.max(...sizes)).toBeLessThanOrEqual(64 * 1024)
    expect(progress.at(-1)).toBe(data.length)
    expect(Buffer.from(await blob.arrayBuffer()).equals(Buffer.from(data))).toBe(true)
  })

  it('cleans a damaged stream and rejects mismatched headers or extra body bytes', async () => {
    const data = new Uint8Array([1, 2, 3]),
      spec = { size: 3, sha256: digest(data) }
    for (const reply of [
      response(new Uint8Array([3, 2, 1]), spec),
      response(data, spec, { etag: `"${'0'.repeat(64)}"` }),
      response(data, spec, { 'content-length': '2' }),
      response(new Uint8Array([1, 2, 3, 4]), spec),
      response(new Uint8Array([1, 2]), spec),
      response(data, spec, { 'content-type': 'text/html' }),
    ]) {
      const target = createBrowserFileSink(spec),
        abort = vi.fn(target.abort)
      await expect(readFileContent(reply, spec, { ...target, abort })).rejects.toMatchObject({
        code: expect.stringMatching(/^FILE_/),
      })
      expect(abort).toHaveBeenCalledTimes(1)
    }
  })

  it('supports empty content and refuses a browser allocation over the preview limit', async () => {
    const data = new Uint8Array(),
      spec = { size: 0, sha256: digest(data) }
    const blob = await readFileContent(response(data, spec), spec, createBrowserFileSink(spec))
    expect(blob.size).toBe(0)
    expect(() =>
      createBrowserFileSink({ size: BROWSER_FILE_LIMIT + 1, sha256: spec.sha256 }),
    ).toThrowError(/桌面/)
    const huge = new Blob()
    Object.defineProperty(huge, 'size', { value: BROWSER_FILE_LIMIT + 1 })
    const read = vi.spyOn(huge, 'arrayBuffer')
    await expect(hashBrowserFile(huge)).rejects.toMatchObject({ code: 'FILE_LIMIT' })
    expect(read).not.toHaveBeenCalled()
  })

  it('aborts a stalled body on the idle deadline without finalizing its target', async () => {
    const spec = { size: 1, sha256: digest(new Uint8Array([1])) }
    const body = new ReadableStream<Uint8Array>({ pull: () => new Promise(() => {}) })
    const template = response(new Uint8Array(), spec)
    const reply = new Response(body, { headers: template.headers })
    const abort = vi.fn(async () => {}),
      complete = vi.fn(async () => {})
    await expect(
      readFileContent(
        reply,
        spec,
        { write: async () => {}, complete, abort },
        { idleTimeoutMs: 20 },
      ),
    ).rejects.toMatchObject({ code: 'FILE_TRANSFER_TIMEOUT' })
    expect(abort).toHaveBeenCalledTimes(1)
    expect(complete).not.toHaveBeenCalled()
  })

  it('cancels a pending sink write promptly and never finalizes it', async () => {
    const data = new Uint8Array([1]),
      spec = { size: 1, sha256: digest(data) },
      controller = new AbortController()
    let entered!: () => void
    const started = new Promise<void>((resolve) => {
      entered = resolve
    })
    const complete = vi.fn(async () => {}),
      abort = vi.fn(async () => {})
    const pending = readFileContent(
      response(data, spec),
      spec,
      {
        write: async () => {
          entered()
          await new Promise(() => {})
        },
        complete,
        abort,
      },
      { signal: controller.signal },
    )
    await started
    controller.abort()
    await expect(pending).rejects.toMatchObject({ code: 'REQUEST_CANCELLED' })
    expect(abort).toHaveBeenCalledTimes(1)
    expect(complete).not.toHaveBeenCalled()
  })

  it('hashes only bounded browser sources and discards cancelled results', async () => {
    const data = new Uint8Array(1024).fill(5),
      controller = new AbortController()
    expect(await hashBrowserFile(new Blob([data]))).toBe(digest(data))
    controller.abort()
    await expect(hashBrowserFile(new Blob([data]), controller.signal)).rejects.toMatchObject({
      code: 'REQUEST_CANCELLED',
    })
  })
})
