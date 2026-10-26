import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useSession } from '../stores/session'
import { ApiError } from '../core/client'
import {
  CredentialRecoveryStore,
  parseToken,
  parseApplication,
  parseReceipt,
  type CredentialOperation,
} from '../core/open-access'
export function useCredentialPage() {
  const session = useSession(),
    identity = computed(() => `${session.settings.apiBaseUrl}|${session.user?.id ?? ''}`),
    pending = ref<CredentialOperation[]>([]),
    recoveryError = ref(''),
    secret = ref<{ kind: 'token' | 'application'; id: string; value: string } | null>(null)
  let epoch = 0,
    alive = true,
    store: CredentialRecoveryStore | null = null
  const controls = new Set<AbortController>(),
    resets = new Set<() => void>()
  const ticket = () => epoch,
    current = (value: number) => alive && value === epoch
  function readPending() {
    try {
      pending.value = store?.read() ?? []
      recoveryError.value = ''
    } catch {
      pending.value = []
      recoveryError.value = '本机恢复记录不可用，请先处理记录后再签发凭据'
    }
  }
  function reset() {
    epoch++
    for (const control of controls) control.abort()
    controls.clear()
    secret.value = null
    store = null
    pending.value = []
    recoveryError.value = ''
    if (session.user && session.settings.apiBaseUrl) {
      try {
        store = new CredentialRecoveryStore(
          localStorage,
          session.settings.apiBaseUrl,
          session.user.id,
        )
        readPending()
      } catch {
        recoveryError.value = '无法保存本机恢复记录，暂时不能签发'
      }
    }
    for (const reset of resets) reset()
  }
  watch(identity, reset, { immediate: true, flush: 'sync' })
  onBeforeUnmount(() => {
    alive = false
    epoch++
    for (const control of controls) control.abort()
    controls.clear()
    secret.value = null
    store = null
  })
  async function request<T = unknown>(
    path: string,
    options: { method?: string; body?: unknown } = {},
  ) {
    const expected = ticket(),
      control = new AbortController()
    controls.add(control)
    try {
      const result = await session.request<T>(path, { ...options, signal: control.signal })
      if (!current(expected)) throw new ApiError('REQUEST_CANCELLED', '当前账号或服务已变化')
      return result
    } finally {
      controls.delete(control)
    }
  }
  function begin(
    kind: CredentialOperation['kind'],
    applicationId: string | null = null,
    version: number | null = null,
  ) {
    if (!store || recoveryError.value) throw new Error('恢复记录不可用，未发送签发请求')
    const row: CredentialOperation = {
      requestId: crypto.randomUUID(),
      kind,
      applicationId,
      version,
      createdAt: new Date().toISOString(),
    }
    store.put(row)
    readPending()
    return row
  }
  function confirmed(id: string, expected: number) {
    if (!current(expected)) return
    try {
      store?.remove(id)
      readPending()
    } catch {
      recoveryError.value = '操作已确认，但本机标记未能移除，请恢复查询'
    }
  }
  function rejected(id: string, expected: number, error: unknown) {
    if (error instanceof ApiError && [401, 403, 404, 422, 429].includes(error.status))
      confirmed(id, expected)
  }
  async function recover(row: CredentialOperation) {
    const expected = ticket()
    let label = ''
    if (row.kind === 'token.create') {
      const result = parseToken((await request(`/api-tokens/by-request/${row.requestId}`)).data)
      if (result.requestId !== row.requestId) throw new Error('回执与签发请求不匹配')
      label = `Token“${result.name}”已建立（${result.status}），秘密不能恢复`
    } else if (row.kind === 'app.create') {
      const result = parseApplication(
        (await request(`/applications/by-request/${row.requestId}`)).data,
      )
      if (result.requestId !== row.requestId) throw new Error('回执与创建请求不匹配')
      label = `应用“${result.name}”已建立，秘密不能恢复`
    } else {
      const result = parseReceipt(
        (await request(`/applications/${row.applicationId}/actions/${row.requestId}`)).data,
      )
      if (
        result.applicationId !== row.applicationId ||
        result.requestId !== row.requestId ||
        result.version !== row.version! + 1
      )
        throw new Error('回执与轮换请求不匹配')
      label = `轮换已完成，凭据版本 ${result.credentialVersion}，秘密不能恢复`
    }
    if (!current(expected)) return ''
    confirmed(row.requestId, expected)
    return label
  }
  function forget(row: CredentialOperation, expected: number) {
    confirmed(row.requestId, expected)
  }
  return {
    session,
    identity,
    pending,
    recoveryError,
    secret,
    ticket,
    current,
    request,
    begin,
    confirmed,
    rejected,
    recover,
    forget,
    onReset: (reset: () => void) => {
      resets.add(reset)
    },
    readPending,
  }
}
