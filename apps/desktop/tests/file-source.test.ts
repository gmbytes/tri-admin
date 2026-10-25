import { it, expect } from 'vitest'
import { createHash } from 'node:crypto'
import { browserUploadSource } from '../src/core/file-source'
it('hashes a bounded selected file and provides immutable fixed size chunks', async () => {
  const bytes = new Uint8Array(4 * 1024 * 1024 + 17).fill(71),
    file = new File([bytes], 'source.bin', { type: 'application/octet-stream' }),
    source = await browserUploadSource(file)
  expect(source.sha256).toBe(createHash('sha256').update(bytes).digest('hex'))
  const chunk = await source.readChunk(1)
  expect(chunk.bytes.size).toBe(17)
  expect(chunk.sha256).toBe(
    createHash('sha256')
      .update(bytes.subarray(4 * 1024 * 1024))
      .digest('hex'),
  )
  expect(JSON.stringify(source)).not.toMatch(/path|handle|password|token/)
  await source.close()
  await expect(source.readChunk(0)).rejects.toMatchObject({ code: 'REQUEST_CANCELLED' })
})
it('retires browser sources on cancellation and rejects invalid indices', async () => {
  const control = new AbortController(),
    source = await browserUploadSource(new File(['data'], 'data.txt'), control.signal)
  await expect(source.readChunk(-1)).rejects.toMatchObject({ code: 'FILE_CHUNK_INVALID' })
  control.abort()
  await expect(source.readChunk(0)).rejects.toMatchObject({ code: 'REQUEST_CANCELLED' })
})
