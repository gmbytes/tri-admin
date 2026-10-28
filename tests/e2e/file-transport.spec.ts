import { readFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { expect, test } from '@playwright/test'

test('browser transport uploads chunks and verifies a scoped download through the real API', async ({
  page,
}) => {
  test.setTimeout(60000)
  const api = process.env.TRI_E2E_API,
    secret = process.env.TRI_E2E_PASSWORD_FILE
  if (!api || !secret) throw new Error('isolated transport fixture is unavailable')
  await page.addInitScript((endpoint) => localStorage.setItem('tri-admin.server', endpoint), api)
  await page.goto('/#/login')
  const proof = await page.evaluate(
    async ({ api, password }) => {
      const clientPath = '/src/core/client.ts',
        filePath = '/src/core/files.ts'
      const { ApiClient } = (await import(
        /* @vite-ignore */ clientPath
      )) as typeof import('../../apps/desktop/src/core/client')
      const { hashBrowserFile, createBrowserFileSink } = (await import(
        /* @vite-ignore */ filePath
      )) as typeof import('../../apps/desktop/src/core/files')
      const client = new ApiClient({
        endpoint: api,
        vault: { load: async () => null, save: async () => {}, clear: async () => {} },
      })
      await client.login('admin', password, false)
      let profile: string | undefined,
        upload: string | undefined,
        file: string | undefined,
        deleted = false
      let proof:
        | { bytes: number; sha256: string; received: number; progress: number; missingCode: string }
        | undefined
      try {
        profile = (await client.request<Array<{ id: string }>>('/files/storage-options?pageSize=1'))
          .data[0]?.id
        if (!profile) throw new Error('Storage fixture unavailable')
        const bytes = new Uint8Array(5 * 1024 * 1024 + 17).fill(43),
          source = new Blob([bytes]),
          sha256 = await hashBrowserFile(source)
        upload = (
          await client.request<{ id: string }>('/uploads', {
            method: 'POST',
            body: {
              requestId: crypto.randomUUID(),
              name: 'transport-proof.bin',
              mediaType: 'application/octet-stream',
              size: source.size,
              sha256,
              storageProfileId: profile,
            },
          })
        ).data.id
        for (const index of [1, 0, 1]) {
          const chunk = source.slice(
            index * 4 * 1024 * 1024,
            Math.min((index + 1) * 4 * 1024 * 1024, source.size),
          )
          const receipt = await client.putChunk<{ index: number; size: number }>(
            upload,
            index,
            chunk,
            await hashBrowserFile(chunk),
          )
          if (receipt.data.index !== index || receipt.data.size !== chunk.size)
            throw new Error('chunk receipt did not match the source')
        }
        const state = await client.request<{ receivedCount: number; receivedBytes: number }>(
          `/uploads/${upload}`,
        )
        if (state.data.receivedCount !== 2 || state.data.receivedBytes !== source.size)
          throw new Error('chunk replay changed durable receipt counts')
        await client.request(`/uploads/${upload}/complete`, { method: 'POST', body: {} })
        const deadline = Date.now() + 15000
        while (Date.now() < deadline) {
          const state = (
            await client.request<{ state: string; fileId: string | null }>(`/uploads/${upload}`)
          ).data
          if (state.state === 'completed') {
            file = state.fileId ?? undefined
            break
          }
          if (['failed', 'cancelled', 'receiving'].includes(state.state))
            throw new Error('publication failed')
          await new Promise((resolve) => setTimeout(resolve, 50))
        }
        if (!file) throw new Error('publication did not complete')
        const metadata = (await client.request<{ size: number; sha256: string }>(`/files/${file}`))
          .data
        if (metadata.size !== source.size || metadata.sha256 !== sha256)
          throw new Error('published metadata changed the contents')
        let progress = 0
        const download = await client.downloadFile(
          file,
          metadata,
          createBrowserFileSink(metadata),
          {
            onProgress: (value) => {
              progress = value
            },
          },
        )
        const downloadedHash = await hashBrowserFile(download)
        if (downloadedHash !== sha256) throw new Error('download hash did not match the source')
        await client.request(`/files/${file}?version=1`, { method: 'DELETE' })
        deleted = true
        let missingCode = ''
        try {
          await client.downloadFile(file, metadata, createBrowserFileSink(metadata))
        } catch (error) {
          missingCode = (error as { code?: string }).code ?? ''
        }
        if (missingCode !== 'NOT_FOUND') throw new Error('deleted file remained downloadable')
        proof = {
          bytes: download.size,
          sha256: downloadedHash,
          received: state.data.receivedCount,
          progress,
          missingCode,
        }
      } finally {
        if (file && !deleted)
          await client.request(`/files/${file}?version=1`, { method: 'DELETE' }).catch(() => {})
        if (upload && !file)
          await client
            .request(`/uploads/${upload}/cancel`, { method: 'POST', body: {} })
            .catch(() => {})
        if (upload)
          await client
            .request(`/uploads/${upload}/cleanup`, { method: 'POST', body: {} })
            .catch(() => {})
        if (!(await client.logout()))
          throw new Error('transport fixture session could not be revoked')
      }
      return proof!
    },
    { api, password: await readFile(secret, 'utf8') },
  )
  expect(proof).toEqual({
    bytes: 5 * 1024 * 1024 + 17,
    sha256: createHash('sha256')
      .update(Buffer.alloc(5 * 1024 * 1024 + 17, 43))
      .digest('hex'),
    received: 2,
    progress: 5 * 1024 * 1024 + 17,
    missingCode: 'NOT_FOUND',
  })
})
