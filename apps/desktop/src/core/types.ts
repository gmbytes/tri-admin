export type DataScope = 'all' | 'self' | 'roles'
export interface UserSummary {
  id: string
  username: string
  displayName: string
  roleIds: string[]
  permissions: string[]
  dataScope: DataScope
  dataScopeRoleIds: string[]
}
export interface Envelope<T> {
  data: T
  meta?: { page: number; pageSize: number; total: number }
}
export interface TokenResponse {
  accessToken: string
  refreshToken: string
  expiresIn: number
  user: UserSummary
}
export interface UserRecord {
  id: string
  username: string
  displayName: string
  roleIds: string[]
  roles: Array<{ id: string; name: string }>
  status: 'active' | 'disabled'
  version: number
  createdAt: string
  updatedAt: string
}
export interface RoleRecord {
  id: string
  name: string
  slug: string
  permissions: string[]
  dataScope: DataScope
  dataScopeRoleIds: string[]
  dataScopeRoles: Array<{ id: string; name: string }>
  protected: boolean
  version: number
  createdAt: string
  updatedAt: string
}
export interface RoleChoice {
  id: string
  name: string
  slug?: string
}
export interface MenuRecord {
  id: string
  label: string
  path: string
  viewKey: string | null
  icon: string | null
  permission: string | null
  parentId: string | null
  sort: number
  enabled: boolean
  version: number
  children?: MenuRecord[]
}
export interface Permission {
  code: string
  label: string
  group: string
}
export interface ApiEndpoint {
  operationId: string
  method: string
  path: string
  group: string
  label: string
  access:
    | 'public'
    | 'authenticated'
    | 'refreshCredential'
    | 'permission'
    | 'openApplication'
    | 'openUser'
    | 'openRefreshCredential'
  requiredPermissions: string[]
  requiredScopes: string[]
  notes: string
}
export interface ApiCatalog {
  schemaVersion: number
  checksum: string
  endpoints: ApiEndpoint[]
}
export type LogKind = 'login' | 'operation' | 'error'
export interface LogRecord {
  id: string
  kind: LogKind
  action: string
  outcome: string
  createdAt: string
  actorId: string | null
  actorName: string | null
  targetId: string | null
  username: string | null
  requestId: string | null
  method: string | null
  route: string | null
  ip: string | null
  status: number | null
  durationMs: number | null
  errorCode: string | null
  message: string | null
}
export interface LogCleanupRequest {
  requestId: string
  kind: LogKind
  before: string
  limit: number
}
export interface LogCleanupResult {
  requestId: string
  kind: LogKind
  before: string
  deletedCount: number
  hasMore: boolean
  completedAt: string
}
