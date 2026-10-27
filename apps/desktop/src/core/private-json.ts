const maximum = 8 * 1024 * 1024
function active(signal?: AbortSignal) {
  if (signal?.aborted) throw new Error('请求已取消')
}
export async function readPrivateJson(response: Response, signal?: AbortSignal): Promise<unknown> {
  let reader: ReadableStreamDefaultReader<Uint8Array> | undefined
  const stop = () => {
    void reader?.cancel().catch(() => undefined)
  }
  try {
    active(signal)
    const policy = (response.headers.get('cache-control') ?? '').toLowerCase().split(/[\s,]+/u)
    if (
      !policy.includes('private') ||
      !policy.includes('no-store') ||
      response.headers.get('content-type')?.split(';')[0]?.trim().toLowerCase() !==
        'application/json'
    )
      throw new Error()
    const header = response.headers.get('content-length')
    if (
      header !== null &&
      (!/^(?:0|[1-9][0-9]*)$/u.test(header) ||
        !Number.isSafeInteger(Number(header)) ||
        Number(header) > maximum)
    )
      throw new Error()
    if (!response.body) throw new Error()
    reader = response.body.getReader()
    signal?.addEventListener('abort', stop, { once: true })
    const chunks: Uint8Array[] = []
    let total = 0,
      count = 0
    while (true) {
      active(signal)
      const part = await reader.read()
      active(signal)
      if (part.done) break
      count++
      if (
        !(part.value instanceof Uint8Array) ||
        count > 8192 ||
        total + part.value.byteLength > maximum
      )
        throw new Error()
      total += part.value.byteLength
      chunks.push(part.value.slice())
    }
    if (header !== null && Number(header) !== total) throw new Error()
    const bytes = new Uint8Array(total)
    let offset = 0
    for (const chunk of chunks) {
      bytes.set(chunk, offset)
      offset += chunk.byteLength
    }
    const value: unknown = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes))
    if (
      !value ||
      typeof value !== 'object' ||
      Array.isArray(value) ||
      Object.keys(value).join('|') !== 'data'
    )
      throw new Error()
    active(signal)
    return (value as { data: unknown }).data
  } catch {
    if (signal?.aborted) throw new Error('请求已取消')
    throw new Error('私有响应格式或大小无效')
  } finally {
    signal?.removeEventListener('abort', stop)
    stop()
    reader?.releaseLock()
  }
}
