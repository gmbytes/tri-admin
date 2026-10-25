export const MAX_FILE_SIZE = 5 * 1024 * 1024 * 1024
export const BROWSER_FILE_LIMIT = 128 * 1024 * 1024
const WRITE_BLOCK = 64 * 1024

export interface FileContentSpec {
  size: number
  sha256: string
}

/** A target keeps writes temporary until complete verifies the supplied size and hash. */
export interface FileSink<T> {
  write(bytes: Uint8Array): Promise<void>
  complete(expected: FileContentSpec): Promise<T>
  abort(): Promise<void>
}

export class FileTransferError extends Error {
  constructor(
    public code: string,
    message: string,
  ) {
    super(message)
    this.name = 'FileTransferError'
  }
}
function cancelled() {
  return new FileTransferError('REQUEST_CANCELLED', '文件操作已取消')
}
function active(signal?: AbortSignal) {
  if (signal?.aborted) throw cancelled()
}
export function validateFileSpec(spec: FileContentSpec) {
  if (
    !spec ||
    typeof spec !== 'object' ||
    !Number.isSafeInteger(spec.size) ||
    spec.size < 0 ||
    spec.size > MAX_FILE_SIZE ||
    typeof spec.sha256 !== 'string' ||
    !/^[a-f0-9]{64}$/.test(spec.sha256)
  )
    throw new FileTransferError('FILE_METADATA_INVALID', '文件长度或摘要无效')
}
async function sha256(bytes: ArrayBuffer) {
  return Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)), (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('')
}
export async function hashBrowserFile(source: Blob, signal?: AbortSignal): Promise<string> {
  active(signal)
  if (!(source instanceof Blob) || source.size > BROWSER_FILE_LIMIT)
    throw new FileTransferError(
      'FILE_LIMIT',
      '浏览器预览最多处理 128 MiB，请使用桌面程序处理更大的文件',
    )
  const bytes = await cancellable(() => source.arrayBuffer(), signal)
  active(signal)
  const digest = await cancellable(() => sha256(bytes), signal)
  active(signal)
  return digest
}

export function createBrowserFileSink(expected: FileContentSpec): FileSink<Blob> {
  validateFileSpec(expected)
  const initial = Object.freeze({ size: expected.size, sha256: expected.sha256 })
  if (initial.size > BROWSER_FILE_LIMIT)
    throw new FileTransferError(
      'FILE_LIMIT',
      '浏览器预览最多处理 128 MiB，请使用桌面程序处理更大的文件',
    )
  let parts: Uint8Array<ArrayBuffer>[] = [],
    length = 0,
    ended = false
  return {
    async write(bytes) {
      if (ended) throw cancelled()
      length += bytes.byteLength
      if (length > initial.size || length > BROWSER_FILE_LIMIT)
        throw new FileTransferError('FILE_INTEGRITY', '下载内容长度不匹配')
      parts.push(new Uint8Array(bytes))
    },
    async complete(spec) {
      if (ended) throw cancelled()
      if (spec.size !== initial.size || spec.sha256 !== initial.sha256 || length !== initial.size)
        throw new FileTransferError('FILE_INTEGRITY', '下载内容长度或摘要不匹配')
      const blob = new Blob(parts, { type: 'application/octet-stream' })
      const checksum = await sha256(await blob.arrayBuffer())
      if (ended) throw cancelled()
      if (checksum !== initial.sha256)
        throw new FileTransferError('FILE_INTEGRITY', '下载内容校验失败，请重新下载')
      ended = true
      parts = []
      return blob
    },
    async abort() {
      ended = true
      parts = []
      length = 0
    },
  }
}

function cancellable<T>(start: () => Promise<T>, signal?: AbortSignal): Promise<T> {
  if (!signal) return Promise.resolve().then(start)
  if (signal.aborted) return Promise.reject(cancelled())
  return new Promise<T>((resolve, reject) => {
    const abort = () => reject(cancelled())
    signal.addEventListener('abort', abort, { once: true })
    Promise.resolve()
      .then(() => {
        active(signal)
        return start()
      })
      .then(resolve, reject)
      .finally(() => signal.removeEventListener('abort', abort))
  })
}
async function readBlock(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  idle: number,
  signal?: AbortSignal,
) {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await cancellable(
      () =>
        Promise.race([
          reader.read(),
          new Promise<never>((_, reject) => {
            timer = setTimeout(
              () =>
                reject(
                  new FileTransferError('FILE_TRANSFER_TIMEOUT', '文件下载长时间无响应，请重试'),
                ),
              idle,
            )
          }),
        ]),
      signal,
    )
  } finally {
    if (timer !== undefined) clearTimeout(timer)
  }
}

export async function readFileContent<T>(
  response: Response,
  expected: FileContentSpec,
  sink: FileSink<T>,
  options: {
    signal?: AbortSignal
    onProgress?: (bytes: number) => void
    idleTimeoutMs?: number
  } = {},
): Promise<T> {
  let reader: ReadableStreamDefaultReader<Uint8Array> | undefined
  let cleanup: Promise<void> | undefined
  const abort = () =>
    (cleanup ??= Promise.resolve()
      .then(() => sink.abort())
      .catch(() => undefined))
  const stop = () => {
    void abort()
    void reader?.cancel().catch(() => undefined)
  }
  options.signal?.addEventListener('abort', stop, { once: true })
  try {
    active(options.signal)
    validateFileSpec(expected)
    const spec = Object.freeze({ size: expected.size, sha256: expected.sha256 })
    const length = response.headers.get('content-length') ?? ''
    if (
      !/^\d+$/.test(length) ||
      Number(length) !== spec.size ||
      response.headers.get('etag') !== `"${spec.sha256}"` ||
      !/^attachment(?:;|$)/i.test(response.headers.get('content-disposition') ?? '') ||
      response.headers.get('content-type')?.split(';')[0]?.trim().toLowerCase() !==
        'application/octet-stream' ||
      !response.body
    )
      throw new FileTransferError('FILE_RESPONSE_INVALID', '文件下载响应与资料不一致')
    reader = response.body.getReader()
    let received = 0
    while (true) {
      const block = await readBlock(reader, options.idleTimeoutMs ?? 30_000, options.signal)
      if (block.done) break
      received += block.value.byteLength
      if (received > spec.size) throw new FileTransferError('FILE_INTEGRITY', '下载内容长度不匹配')
      for (let offset = 0; offset < block.value.byteLength; offset += WRITE_BLOCK) {
        active(options.signal)
        await cancellable(
          () => sink.write(block.value.subarray(offset, offset + WRITE_BLOCK)),
          options.signal,
        )
      }
      active(options.signal)
      options.onProgress?.(received)
    }
    if (received !== spec.size) throw new FileTransferError('FILE_INTEGRITY', '下载内容不完整')
    active(options.signal)
    const result = await cancellable(() => sink.complete(spec), options.signal)
    active(options.signal)
    return result
  } catch (error) {
    stop()
    await abort()
    if (options.signal?.aborted) throw cancelled()
    if (error instanceof FileTransferError) throw error
    throw new FileTransferError('FILE_TRANSFER_FAILED', '文件接收或写入失败，请重试')
  } finally {
    options.signal?.removeEventListener('abort', stop)
    if (reader) reader.releaseLock()
    else void response.body?.cancel().catch(() => undefined)
  }
}
