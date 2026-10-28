import { expect, it } from 'vitest'
import {
  filterNavigation,
  viewKeys,
  filterPermissions,
  filterPermissionCodes,
  filterApiEndpoints,
} from '../src/core/navigation'
import type { MenuRecord } from '../src/core/types'
const menu = (path: string, viewKey: string | null, children: MenuRecord[] = []): MenuRecord => ({
  id: path,
  label: path,
  path,
  viewKey,
  children,
  icon: null,
  permission: null,
  parentId: null,
  sort: 0,
  enabled: true,
  version: 1,
})
it('retains exactly the seven approved page types including file management', () => {
  expect([...viewKeys]).toEqual([
    'dashboard',
    'users',
    'roles',
    'menus',
    'permissions',
    'audit',
    'files',
  ])
  expect(
    filterNavigation([
      menu('/files', 'files'),
      menu('/dictionaries', 'dictionaries'),
      menu('/ai', 'ai'),
    ]).map((row) => row.path),
  ).toEqual(['/files'])
})
it('drops empty retired groups while preserving current nested navigation without mutating the response', () => {
  const source = [
    menu('/group', null, [menu('/users', 'users'), menu('/codegen', 'codegen')]),
    menu('/old-group', null, [menu('/mcp', 'mcp')]),
  ]
  const result = filterNavigation(source)
  expect(result).toHaveLength(1)
  expect(result[0]?.children?.map((row) => row.path)).toEqual(['/users'])
  expect(source[0]?.children).toHaveLength(2)
})
it('offers only permissions belonging to the approved modules', () => {
  expect(
    filterPermissions([
      { code: 'files.write', group: 'files', label: '文件管理' },
      { code: 'ai.invoke', group: 'ai', label: '模型调用' },
      { code: 'settings.write', group: 'settings', label: '配置中心' },
    ]).map((row) => row.code),
  ).toEqual(['files.write'])
})
it('removes retired grants from role forms while preserving the existing wildcard', () => {
  expect(
    filterPermissionCodes(['*', 'users.read', 'dictionaries.read', 'mcp.invoke', 'files.read']),
  ).toEqual(['*', 'users.read', 'files.read'])
})
it('does not expose retired API definitions through the permissions page', () => {
  const entries = [
    { operationId: 'auth.login' },
    { operationId: 'files.list' },
    { operationId: 'settings.service.get' },
    { operationId: 'workflows.runs.create' },
  ] as import('../src/core/types').ApiEndpoint[]
  expect(filterApiEndpoints(entries).map((row) => row.operationId)).toEqual([
    'auth.login',
    'files.list',
  ])
})
