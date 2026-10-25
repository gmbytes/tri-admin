export const MAX_CONFIGURATION_BYTES = 8 * 1024 * 1024
export interface ConfigurationDownload {
  bytes: Uint8Array
  filename: string
  checksum: string
}

export function decodeConfigurationFile(bytes: ArrayBuffer): string {
  if (bytes.byteLength > MAX_CONFIGURATION_BYTES) throw new Error('配置包最多 8 MiB')
  let text: string
  try {
    text = new TextDecoder('utf-8', { fatal: true }).decode(bytes)
  } catch {
    throw new Error('配置包文件必须为有效的 UTF-8')
  }
  parseConfigurationPackage(text)
  return text
}

export function parseConfigurationPackage(text: string): unknown {
  if (new TextEncoder().encode(text).byteLength > MAX_CONFIGURATION_BYTES)
    throw new Error('配置包最多 8 MiB')
  let value: unknown
  try {
    value = JSON.parse(text)
  } catch {
    throw new Error('请输入有效的配置包 JSON')
  }
  if (
    !value ||
    typeof value !== 'object' ||
    (value as { format?: unknown }).format !== 'tri-admin/config-bundle' ||
    (value as { schemaVersion?: unknown }).schemaVersion !== 1
  )
    throw new Error('仅支持 tri-admin/config-bundle 新格式 schemaVersion=1')
  const pending: Array<{ value: unknown; depth: number }> = [{ value, depth: 0 }]
  while (pending.length) {
    const entry = pending.pop()!
    if (entry.depth > 32) throw new Error('配置包嵌套层级过深')
    if (
      typeof entry.value === 'number' &&
      (!Number.isFinite(entry.value) || Math.abs(entry.value) > Number.MAX_SAFE_INTEGER)
    )
      throw new Error('配置包中的数字超出安全范围，请使用字符串表示大整数')
    if (entry.value && typeof entry.value === 'object')
      for (const child of Object.values(entry.value))
        pending.push({ value: child, depth: entry.depth + 1 })
  }
  return value
}

export async function readConfigurationDownload(
  response: Response,
): Promise<ConfigurationDownload> {
  const filename = /^attachment; filename="(tri-admin-config-[A-Za-z0-9._-]+\.json)"$/.exec(
    response.headers.get('content-disposition') ?? '',
  )?.[1]
  const checksum = /^"([a-f0-9]{64})"$/.exec(response.headers.get('etag') ?? '')?.[1]
  if (
    !filename ||
    !checksum ||
    response.headers.get('content-type')?.split(';')[0]?.trim().toLowerCase() !==
      'application/json' ||
    Number(response.headers.get('content-length')) > MAX_CONFIGURATION_BYTES ||
    !response.body
  ) {
    await response.body?.cancel()
    throw new Error('配置包下载响应无效或超过 8 MiB')
  }
  const reader = response.body.getReader(),
    chunks: Uint8Array[] = []
  let length = 0
  try {
    while (true) {
      const chunk = await reader.read()
      if (chunk.done) break
      length += chunk.value.byteLength
      if (length > MAX_CONFIGURATION_BYTES) {
        await reader.cancel()
        throw new Error('配置包最多 8 MiB')
      }
      chunks.push(chunk.value)
    }
  } finally {
    reader.releaseLock()
  }
  const bytes = new Uint8Array(length)
  let offset = 0
  for (const chunk of chunks) {
    bytes.set(chunk, offset)
    offset += chunk.byteLength
  }
  const digest = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)), (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('')
  if (digest !== checksum) throw new Error('配置包校验失败，请重新下载')
  return { bytes, filename, checksum }
}
