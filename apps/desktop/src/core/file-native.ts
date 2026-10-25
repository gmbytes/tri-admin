import { ApiError } from './client'
import {
  FILE_CHUNK_SIZE,
  FileTransferError,
  hashBrowserFile,
  validateFileSpec,
  type FileContentSpec,
  type FileSink,
  type UploadSource,
} from './files'
export interface NativeFileBridge {
  invoke<T>(
    name: string,
    args?: Record<string, unknown> | Uint8Array,
    options?: { headers?: Record<string, string> },
  ): Promise<T>
  channel(onMessage: (bytes: number) => void): unknown
}
export interface NativeSaveResult extends FileContentSpec {
  kind: 'native'
  cleanupPending: boolean
}
interface FileOptions {
  signal?: AbortSignal
  onProgress?: (bytes: number) => void
}
function cancelled() {
  return new ApiError('REQUEST_CANCELLED', '文件操作已取消')
}
function changed() {
  return new ApiError('SESSION_CHANGED', '当前会话已切换')
}
function failure(error: unknown): ApiError {
  if (error instanceof ApiError) return error
  if (error instanceof FileTransferError) return new ApiError(error.code, error.message)
  const value = error as { code?: unknown; message?: unknown } | null
  if (
    value &&
    typeof value.code === 'string' &&
    /^(FILE_[A-Z_]+|REQUEST_CANCELLED)$/.test(value.code) &&
    typeof value.message === 'string'
  )
    return new ApiError(value.code, value.message.slice(0, 300))
  return new ApiError('FILE_IO', '本地文件操作失败，请重试')
}

export class NativeFileAccess {
  private epoch = 0
  private barrier: Promise<void> = Promise.resolve()
  constructor(private bridge: NativeFileBridge) {}
  private async call<T>(
    name: string,
    args?: Record<string, unknown> | Uint8Array,
    options?: { headers?: Record<string, string> },
  ): Promise<T> {
    try {
      return await this.bridge.invoke<T>(name, args, options)
    } catch (error) {
      throw failure(error)
    }
  }
  invalidate(): Promise<void> {
    this.epoch++
    const reset = () => this.call<void>('file_cancel_all')
    this.barrier = this.barrier.then(reset, reset)
    return this.barrier
  }
  private async begin(kind: 'source' | 'download', signal?: AbortSignal) {
    const epoch = this.epoch
    await this.barrier
    if (epoch !== this.epoch) throw changed()
    if (signal?.aborted) throw cancelled()
    const handle = await this.call<string>('file_reserve', { kind })
    if (
      typeof handle !== 'string' ||
      !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(handle)
    )
      throw new ApiError('FILE_HANDLE_INVALID', '本地文件句柄无效')
    let closed = false,
      finished = false,
      closing: Promise<void> | undefined
    const onAbort = () => {
      void close()
    }
    const close = () => {
      if (finished) return Promise.resolve()
      closed = true
      signal?.removeEventListener('abort', onAbort)
      return (closing ??= this.call<void>('file_cancel', { handle }).catch(() => undefined))
    }
    const check = (extra?: AbortSignal) => {
      if (epoch !== this.epoch) throw changed()
      if (closed || signal?.aborted || extra?.aborted) throw cancelled()
    }
    signal?.addEventListener('abort', onAbort, { once: true })
    try {
      check()
    } catch (error) {
      await close()
      throw error
    }
    return {
      handle,
      check,
      close,
      done: () => {
        finished = true
        closed = true
        signal?.removeEventListener('abort', onAbort)
      },
    }
  }
  async openSource(options: FileOptions = {}): Promise<UploadSource | null> {
    const job = await this.begin('source', options.signal)
    try {
      const progress = this.bridge.channel((bytes) => {
        try {
          job.check()
          if (Number.isSafeInteger(bytes) && bytes >= 0) options.onProgress?.(bytes)
        } catch {
          /* Discard messages from retired jobs. */
        }
      })
      const info = await this.call<{ name: string; size: number; sha256: string } | null>(
        'file_select_source',
        { handle: job.handle, progress },
      )
      job.check()
      if (info === null) {
        await job.close()
        return null
      }
      if (
        !info ||
        Object.keys(info).sort().join(',') !== 'name,sha256,size' ||
        typeof info.name !== 'string' ||
        !info.name ||
        info.name.length > 1024 ||
        /[\\/\x00-\x1f]/.test(info.name)
      )
        throw new ApiError('FILE_METADATA_INVALID', '本地源文件信息无效')
      validateFileSpec(info)
      const metadata = Object.freeze({ name: info.name, size: info.size, sha256: info.sha256 })
      return {
        ...metadata,
        mediaType: 'application/octet-stream',
        readChunk: async (index, signal) => {
          job.check(signal)
          if (
            !Number.isInteger(index) ||
            index < 0 ||
            index >= Math.ceil(metadata.size / FILE_CHUNK_SIZE)
          )
            throw new ApiError('FILE_CHUNK_INVALID', '分片索引无效')
          const bytes = await this.call<ArrayBuffer>('file_read_chunk', {
            handle: job.handle,
            index,
          })
          job.check(signal)
          if (
            !(bytes instanceof ArrayBuffer) ||
            bytes.byteLength !== Math.min(FILE_CHUNK_SIZE, metadata.size - index * FILE_CHUNK_SIZE)
          )
            throw new ApiError('FILE_RESPONSE_INVALID', '本地文件分片信息无效')
          const blob = new Blob([bytes]),
            sha256 = await hashBrowserFile(blob, signal)
          job.check(signal)
          return { bytes: blob, sha256 }
        },
        close: job.close,
      }
    } catch (error) {
      await job.close()
      throw failure(error)
    }
  }
  async chooseTarget(
    name: string,
    expected: FileContentSpec,
    options: FileOptions = {},
  ): Promise<FileSink<NativeSaveResult> | null> {
    validateFileSpec(expected)
    if (typeof name !== 'string' || !name || name.length > 1024)
      throw new ApiError('FILE_METADATA_INVALID', '保存文件名无效')
    const spec = Object.freeze({ size: expected.size, sha256: expected.sha256 }),
      job = await this.begin('download', options.signal)
    try {
      const selected = await this.call<boolean>('file_select_download', {
        handle: job.handle,
        name,
        spec,
      })
      job.check()
      if (selected === false) {
        await job.close()
        return null
      }
      if (selected !== true) throw new ApiError('FILE_RESPONSE_INVALID', '本地文件选择结果无效')
      let parts: Uint8Array<ArrayBuffer>[] = [],
        buffered = 0,
        received = 0
      const discard = () => {
        parts = []
        buffered = 0
      }
      const abort = () => {
        discard()
        options.signal?.removeEventListener('abort', onAbort)
        return job.close()
      }
      const onAbort = () => {
        void abort()
      }
      options.signal?.addEventListener('abort', onAbort, { once: true })
      const flush = async () => {
        job.check()
        if (!buffered) return
        const bytes = new Uint8Array(buffered)
        let offset = 0
        for (const part of parts) {
          bytes.set(part, offset)
          offset += part.length
        }
        discard()
        await this.call<void>('file_write', bytes, { headers: { 'x-tri-file-handle': job.handle } })
        job.check()
      }
      return {
        write: async (bytes) => {
          job.check()
          if (!(bytes instanceof Uint8Array) || bytes.byteLength > 64 * 1024)
            throw new ApiError('FILE_BLOCK_INVALID', '文件写入块超过 64 KiB')
          if (received + bytes.byteLength > spec.size)
            throw new ApiError('FILE_INTEGRITY', '文件长度不匹配')
          if (buffered + bytes.byteLength > 1024 * 1024) await flush()
          parts.push(new Uint8Array(bytes))
          buffered += bytes.byteLength
          received += bytes.byteLength
          if (buffered === 1024 * 1024) await flush()
          job.check()
        },
        complete: async (value) => {
          try {
            job.check()
            if (value.size !== spec.size || value.sha256 !== spec.sha256)
              throw new ApiError('FILE_METADATA_INVALID', '文件校验信息已变更')
            if (received !== spec.size) throw new ApiError('FILE_INTEGRITY', '文件内容不完整')
            await flush()
            const result = await this.call<{
              size: number
              sha256: string
              cleanupPending: boolean
            }>('file_complete', { handle: job.handle })
            job.check()
            if (
              !result ||
              Object.keys(result).sort().join(',') !== 'cleanupPending,sha256,size' ||
              result.size !== spec.size ||
              result.sha256 !== spec.sha256 ||
              typeof result.cleanupPending !== 'boolean'
            )
              throw new ApiError('FILE_RESPONSE_INVALID', '本地文件保存结果无效')
            job.done()
            options.signal?.removeEventListener('abort', onAbort)
            return { kind: 'native', ...result }
          } catch (error) {
            await abort()
            throw failure(error)
          }
        },
        abort,
      }
    } catch (error) {
      await job.close()
      throw failure(error)
    }
  }
}
