import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { ApiClient, ApiError } from '../core/client'
import { normalizeEndpoint } from '../core/endpoint'
import type { Envelope, MenuRecord, UserSummary } from '../core/types'
import { platform, type Settings } from '../platform'

export const useSession = defineStore('session', () => {
  const user = ref<UserSummary | null>(null),
    navigation = ref<MenuRecord[]>([])
  const application = ref({ displayName: 'TRI ADMIN', version: '' })
  const settings = ref<Settings>({ apiBaseUrl: '', rememberSession: false })
  const initializing = ref(true),
    busy = ref(false),
    notice = ref('')
  const connection = ref<'checking' | 'ready' | 'offline' | 'unconfigured'>('unconfigured')
  let client: ApiClient | null = null,
    initialization: Promise<void> | null = null
  function createClient(endpoint: string) {
    client = new ApiClient({
      endpoint,
      vault: platform.vault,
      onChange: (value) => {
        user.value = value
        if (!value) {
          navigation.value = []
          application.value = { displayName: 'TRI ADMIN', version: '' }
        }
      },
    })
  }
  function can(code: string) {
    return !!user.value?.permissions.some((value) => value === '*' || value === code)
  }
  function flatten(nodes: MenuRecord[]): MenuRecord[] {
    return nodes.flatMap((node) => [node, ...flatten(node.children ?? [])])
  }
  const firstPath = computed(
    () => flatten(navigation.value).find((node) => node.viewKey)?.path ?? '/profile',
  )
  function findMenu(path: string) {
    return flatten(navigation.value).find((node) => node.path === path)
  }
  async function probe() {
    if (!settings.value.apiBaseUrl) {
      connection.value = 'unconfigured'
      return
    }
    connection.value = 'checking'
    const endpoint = settings.value.apiBaseUrl
    try {
      const result = await fetch(`${new URL(endpoint).origin}/health/ready`, {
        signal: AbortSignal.timeout(5000),
      })
      if (endpoint === settings.value.apiBaseUrl) connection.value = result.ok ? 'ready' : 'offline'
    } catch {
      if (endpoint === settings.value.apiBaseUrl) connection.value = 'offline'
    }
  }
  async function loadNavigation() {
    const source = client,
      actor = user.value?.id
    if (source && actor) {
      const [menus, info] = await Promise.all([
        source.request<MenuRecord[]>('/navigation'),
        source.request<{ displayName: string; version: string }>('/application'),
      ])
      if (client === source && user.value?.id === actor) {
        navigation.value = menus.data
        application.value = info.data
      }
    }
  }
  async function loadApplication() {
    const source = client,
      actor = user.value?.id
    if (source && actor) {
      const result = await source.request<{ displayName: string; version: string }>('/application')
      if (client === source && user.value?.id === actor) application.value = result.data
    }
  }
  async function syncProfile() {
    if (client && user.value) {
      user.value = (await client.request<UserSummary>('/auth/me')).data
      await loadNavigation()
    }
  }
  function initialize() {
    return (initialization ??= (async () => {
      try {
        settings.value = await platform.loadSettings()
        if (settings.value.apiBaseUrl) {
          createClient(settings.value.apiBaseUrl)
          if (platform.native && settings.value.rememberSession) {
            try {
              await client!.restore()
              if (user.value) await loadNavigation()
            } catch {
              notice.value = '登录状态已失效或暂时无法恢复，请重新登录'
              await client!.clearSession().catch(() => undefined)
              settings.value.rememberSession = false
            }
          }
          void probe()
        }
      } catch (error) {
        notice.value = error instanceof Error ? error.message : '无法读取客户端设置'
      } finally {
        initializing.value = false
      }
    })())
  }
  async function login(username: string, password: string, remember: boolean) {
    if (!client) throw new ApiError('SERVER_REQUIRED', '请先设置服务器地址')
    busy.value = true
    try {
      await client.login(username, password, remember && platform.native)
      settings.value.rememberSession = remember && platform.native
      await loadNavigation()
      connection.value = 'ready'
      notice.value = ''
    } finally {
      busy.value = false
    }
  }
  async function logout() {
    busy.value = true
    try {
      const revoked = await client?.logout()
      settings.value.rememberSession = false
      if (revoked === false) notice.value = '本机已退出，服务器暂不可达'
    } finally {
      busy.value = false
    }
  }
  async function changeServer(raw: string) {
    const endpoint = normalizeEndpoint(raw)
    if (endpoint === settings.value.apiBaseUrl) return
    busy.value = true
    try {
      await client?.logout()
      settings.value = await platform.setServer(endpoint)
      createClient(endpoint)
      notice.value = ''
      void probe()
    } finally {
      busy.value = false
    }
  }
  async function request<T>(
    path: string,
    options: { method?: string; body?: unknown } = {},
  ): Promise<Envelope<T>> {
    if (!client) throw new ApiError('SERVER_REQUIRED', '请先设置服务器地址')
    return client.request<T>(path, options)
  }
  async function passwordChanged() {
    await client?.clearSession()
    settings.value.rememberSession = false
  }
  async function downloadConfiguration(path: string) {
    if (!client) throw new ApiError('SERVER_REQUIRED', '请先设置服务器地址')
    return client.downloadConfiguration(path)
  }
  return {
    user,
    navigation,
    application,
    settings,
    initializing,
    busy,
    notice,
    connection,
    firstPath,
    can,
    findMenu,
    initialize,
    probe,
    login,
    logout,
    changeServer,
    request,
    downloadConfiguration,
    syncProfile,
    loadNavigation,
    loadApplication,
    passwordChanged,
    native: platform.native,
  }
})
