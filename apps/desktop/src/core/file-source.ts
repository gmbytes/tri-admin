import { ApiError } from './client'
import { validMediaType } from './file-models'
import { FILE_CHUNK_SIZE, hashBrowserFile, type UploadSource } from './files'
export async function browserUploadSource(file: File, signal?: AbortSignal): Promise<UploadSource> {
  const sha256 = await hashBrowserFile(file, signal),
    size = file.size,
    name = file.name,
    mediaType = validMediaType(file.type) ? file.type : 'application/octet-stream'
  let content: Blob | null = file
  const check = (extra?: AbortSignal) => {
    if (!content || signal?.aborted || extra?.aborted)
      throw new ApiError('REQUEST_CANCELLED', '文件操作已取消')
  }
  const close = async () => {
    content = null
    signal?.removeEventListener('abort', retire)
  }
  const retire = () => {
    void close()
  }
  signal?.addEventListener('abort', retire, { once: true })
  check()
  return {
    name,
    size,
    sha256,
    mediaType,
    readChunk: async (index, extra) => {
      check(extra)
      if (!Number.isInteger(index) || index < 0 || index >= Math.ceil(size / FILE_CHUNK_SIZE))
        throw new ApiError('FILE_CHUNK_INVALID', '分片索引无效')
      const bytes = content!.slice(
          index * FILE_CHUNK_SIZE,
          Math.min((index + 1) * FILE_CHUNK_SIZE, size),
        ),
        digest = await hashBrowserFile(bytes, extra)
      check(extra)
      return { bytes, sha256: digest }
    },
    close,
  }
}
