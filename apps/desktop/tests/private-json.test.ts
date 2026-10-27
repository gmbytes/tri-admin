import { it, expect } from 'vitest'
import { readPrivateJson } from '../src/core/private-json'
const headers = { 'content-type': 'application/json', 'cache-control': 'private, no-store' }
it('decodes fragmented UTF-8 private envelopes without changing the content', async () => {
  const bytes = new TextEncoder().encode(JSON.stringify({ data: { input: 'PRIVATE-你好🌍' } }))
  for (let split = 1; split < bytes.length; split++) {
    const stream = new ReadableStream<Uint8Array>({
      start(c) {
        c.enqueue(bytes.slice(0, split))
        c.enqueue(bytes.slice(split))
        c.close()
      },
    })
    expect(await readPrivateJson(new Response(stream, { headers }))).toEqual({
      input: 'PRIVATE-你好🌍',
    })
  }
})
it('rejects unbounded streams and dishonest content length before publishing data', async () => {
  await expect(
    readPrivateJson(
      new Response('{}', {
        headers: { ...headers, 'content-length': String(8 * 1024 * 1024 + 1) },
      }),
    ),
  ).rejects.toThrow()
  let cancelled = false
  const stream = new ReadableStream<Uint8Array>({
    start(c) {
      c.enqueue(new Uint8Array(8 * 1024 * 1024))
      c.enqueue(new Uint8Array([1]))
    },
    cancel() {
      cancelled = true
    },
  })
  await expect(readPrivateJson(new Response(stream, { headers }))).rejects.toThrow()
  expect(cancelled).toBe(true)
  await expect(
    readPrivateJson(new Response('{"data":1}', { headers: { ...headers, 'content-length': '2' } })),
  ).rejects.toThrow()
})
it('rejects invalid bytes, JSON and cache policy without echoing private bodies', async () => {
  for (const response of [
    new Response(new Uint8Array([0xff]), { headers }),
    new Response('{"data":"PRIVATE-BODY" bad}', { headers }),
    new Response('{"data":1}', { headers: { 'content-type': 'text/html' } }),
  ]) {
    try {
      await readPrivateJson(response)
      throw new Error('expected rejection')
    } catch (error) {
      expect(String(error)).not.toContain('PRIVATE-BODY')
      expect(String(error)).not.toContain('expected rejection')
    }
  }
})
it('cancels a stalled reader when its original request is aborted', async () => {
  let cancelled = false
  const signal = new AbortController()
  const stream = new ReadableStream<Uint8Array>({
    cancel() {
      cancelled = true
    },
  })
  const task = readPrivateJson(new Response(stream, { headers }), signal.signal)
  signal.abort()
  await expect(task).rejects.toThrow()
  expect(cancelled).toBe(true)
})
