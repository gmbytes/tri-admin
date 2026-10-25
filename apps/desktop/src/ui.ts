import { ElMessage } from 'element-plus'
import { ApiError } from './core/client'
import type { DataScope } from './core/types'
export function scopeText(scope: DataScope | undefined) {
  return scope === 'all' ? '全部数据' : scope === 'roles' ? '自身及指定角色' : '自身数据'
}
export function errorText(error: unknown) {
  return error instanceof Error ? error.message : '操作失败，请稍后重试'
}
export function notifyError(error: unknown) {
  if (error instanceof ApiError && error.code === 'SESSION_CHANGED') return
  ElMessage.error(errorText(error))
}
export function dateText(value: string) {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '—' : date.toLocaleString('zh-CN', { hour12: false })
}
