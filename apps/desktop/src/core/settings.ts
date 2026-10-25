export type IntegrationKind = 'mongodb' | 'localStorage' | 's3' | 'smtp' | 'http'
export type Integration =
  | {
      kind: 'mongodb'
      hosts: { host: string; port: number }[]
      database: string
      authDatabase: string
      replicaSet: string | null
      tls: boolean
      directConnection: boolean
    }
  | { kind: 'localStorage'; subdirectory: string }
  | {
      kind: 's3'
      endpoint: string
      bucket: string
      region: string
      prefix: string
      pathStyle: boolean
    }
  | {
      kind: 'smtp'
      host: string
      port: number
      security: 'tls' | 'startTls' | 'plain'
      fromAddress: string
    }
  | { kind: 'http'; baseUrl: string }
export type IntegrationSecret =
  | { kind: 'mongodb' | 'smtp'; username: string; password: string }
  | { kind: 's3'; accessKeyId: string; secretAccessKey: string; sessionToken: string | null }
  | { kind: 'http'; headerName: string; value: string }
export interface ConnectionCheck {
  id: string
  profileId: string
  profileVersion: number
  actorId: string
  kind: IntegrationKind
  success: boolean
  scope: string
  errorCode: string | null
  upstreamStatus: number | null
  durationMs: number
  startedAt: string
  completedAt: string
  requestId: string | null
  isCurrent: boolean
}
export interface IntegrationProfile {
  id: string
  name: string
  code: string
  description: string
  enabled: boolean
  configuration: Integration
  credentials: { configured: boolean; keyId: string | null; keyAvailable: boolean }
  version: number
  createdAt: string
  updatedAt: string
  lastCheck: ConnectionCheck | null
}
export interface ProfileDraft {
  name: string
  code: string
  description: string
  enabled: boolean
  configuration: Integration
  secretMode: 'keep' | 'clear' | 'replace'
  secret: IntegrationSecret | null
}
export interface ServiceSettings {
  displayName: string
  loginLimitPerIpOverride: number | null
  effectiveLoginLimitPerIp: number
  storageProfileId: string | null
  mailProfileId: string | null
  version: number
  updatedAt: string
}
export interface RuntimeSettings {
  configuredBind: string
  databaseName: string | null
  corsOrigins: string[]
  storageRoot: string
  accessTokenSeconds: number
  sessionDays: number
  bootLoginLimitPerIp: number
  databaseConfigured: boolean
  settingsKeysConfigured: boolean
  settingsActiveKeyId: string | null
  settingsDecryptionKeyIds: string[]
}
export const integrationKinds: { value: IntegrationKind; label: string }[] = [
  { value: 'mongodb', label: 'MongoDB' },
  { value: 'localStorage', label: '本地存储' },
  { value: 's3', label: 'S3 对象存储' },
  { value: 'smtp', label: 'SMTP 邮件' },
  { value: 'http', label: 'HTTP 服务' },
]
export function integrationLabel(kind: IntegrationKind) {
  return integrationKinds.find((row) => row.value === kind)?.label ?? kind
}
export const scopeLabels: Record<string, string> = {
  httpHead: 'HTTP HEAD',
  mongodbPing: 'MongoDB ping',
  s3BucketHead: 'S3 bucket HEAD',
  smtpSession: 'SMTP 会话',
  localWriteReadDelete: '本地写/读/删除',
}
function canonical(value: unknown): string {
  if (Array.isArray(value)) return '[' + value.map(canonical).join(',') + ']'
  if (value && typeof value === 'object')
    return (
      '{' +
      Object.entries(value)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, item]) => JSON.stringify(key) + ':' + canonical(item))
        .join(',') +
      '}'
    )
  return JSON.stringify(value)
}
export function profilePayload(draft: ProfileDraft, original: IntegrationProfile | null) {
  if (!draft.name.trim() || !draft.code.trim()) throw new Error('请填写配置名称和配置标识')
  if (original && original.configuration.kind !== draft.configuration.kind)
    throw new Error('配置类型不能修改')
  const changed = original && canonical(original.configuration) !== canonical(draft.configuration)
  if (changed && ['localStorage', 's3'].includes(draft.configuration.kind))
    throw new Error('存储位置不能修改，请新建配置后切换默认绑定')
  if (changed && original.credentials.configured && draft.secretMode === 'keep')
    throw new Error('连接配置已改变，请明确替换或清除凭据')
  if (
    draft.secretMode === 'replace' &&
    (!draft.secret || draft.secret.kind !== draft.configuration.kind)
  )
    throw new Error('请填写与配置类型相符的完整凭据')
  if (draft.configuration.kind === 'localStorage' && draft.secretMode === 'replace')
    throw new Error('本地存储无需凭据')
  return {
    name: draft.name,
    description: draft.description,
    enabled: draft.enabled,
    configuration: draft.configuration,
    ...(original ? { version: original.version } : { code: draft.code }),
    ...(draft.secretMode === 'clear'
      ? { secret: null }
      : draft.secretMode === 'replace'
        ? { secret: draft.secret }
        : {}),
  }
}
export function servicePayload(draft: ServiceSettings, original: ServiceSettings) {
  const body: Record<string, unknown> = { version: original.version }
  for (const key of [
    'displayName',
    'loginLimitPerIpOverride',
    'storageProfileId',
    'mailProfileId',
  ] as const) {
    if (draft[key] !== original[key]) body[key] = draft[key]
  }
  return body
}
