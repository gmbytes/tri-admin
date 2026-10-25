export type DataScope = 'all' | 'self'
export interface UserSummary {
  id: string
  username: string
  displayName: string
  roleIds: string[]
  permissions: string[]
  dataScope: DataScope
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
  protected: boolean
  version: number
  createdAt: string
  updatedAt: string
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
export interface DictionaryRecord {
  id: string
  name: string
  key: string
  description: string
  enabled: boolean
  version: number
  createdAt: string
  updatedAt: string
}
export interface DictionaryItem {
  id: string
  dictionaryId: string
  label: string
  value: string
  sort: number
  enabled: boolean
  version: number
  createdAt: string
  updatedAt: string
}
