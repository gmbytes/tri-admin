import type { MenuRecord, Permission, ApiEndpoint } from './types'
export const viewKeys = [
  'dashboard',
  'users',
  'roles',
  'menus',
  'permissions',
  'audit',
  'files',
] as const
const allowed = new Set<string>(viewKeys)
const permissionCodes = new Set([
  'dashboard.read',
  'users.read',
  'users.write',
  'users.resetPassword',
  'roles.read',
  'roles.write',
  'roles.assign',
  'menus.read',
  'menus.write',
  'permissions.read',
  'audit.read',
  'audit.delete',
  'files.read',
  'files.write',
])
export function filterPermissions(rows: Permission[]): Permission[] {
  return rows.filter((row) => permissionCodes.has(row.code))
}
export function filterPermissionCodes(codes: string[]): string[] {
  return codes.filter((code) => code === '*' || permissionCodes.has(code))
}
const apiGroups = new Set([
  'health',
  'auth',
  'users',
  'roles',
  'menus',
  'navigation',
  'permissions',
  'apiCatalog',
  'application',
  'logs',
  'files',
  'uploads',
  'fileCategories',
])
export function filterApiEndpoints(rows: ApiEndpoint[]): ApiEndpoint[] {
  return rows.filter((row) => apiGroups.has(row.operationId.split('.')[0] ?? ''))
}
export function filterNavigation(nodes: MenuRecord[]): MenuRecord[] {
  return nodes.flatMap((node) => {
    const children = filterNavigation(node.children ?? [])
    if (node.viewKey ? !allowed.has(node.viewKey) : children.length === 0) return []
    return [{ ...node, children }]
  })
}
