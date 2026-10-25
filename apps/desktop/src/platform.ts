import { invoke, isTauri, Channel } from '@tauri-apps/api/core'
import { ApiError, type SessionVault } from './core/client'
import { normalizeEndpoint } from './core/endpoint'
import { NativeFileAccess } from './core/file-native'

export interface Settings {
  apiBaseUrl: string
  rememberSession: boolean
}
const native = isTauri()
const files = native
  ? new NativeFileAccess({
      invoke: (name, args, options) =>
        invoke(name, args, options?.headers ? { headers: options.headers } : undefined),
      channel: (onMessage) => {
        const channel = new Channel<number>()
        channel.onmessage = onMessage
        return channel
      },
    })
  : null
async function command<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(name, args)
  } catch (failure) {
    const value = failure as { code?: string; message?: string }
    throw new ApiError(
      value?.code ?? 'CLIENT_STORAGE_ERROR',
      value?.message ?? '客户端设置或会话存储暂不可用',
    )
  }
}
let browserSettings: Settings = {
  apiBaseUrl: import.meta.env.DEV ? 'http://127.0.0.1:18088/api/v1' : '',
  rememberSession: false,
}
const memory = new Map<string, string>()
export const platform = {
  native,
  files,
  async loadSettings(): Promise<Settings> {
    if (native) return command<Settings>('load_settings')
    try {
      const endpoint = localStorage.getItem('tri-admin.server')
      if (endpoint) browserSettings.apiBaseUrl = normalizeEndpoint(endpoint)
    } catch {
      /* Browser preview storage can be unavailable. */
    }
    return { ...browserSettings }
  },
  async setServer(endpoint: string): Promise<Settings> {
    if (native) return command<Settings>('set_server', { endpoint })
    browserSettings = { apiBaseUrl: normalizeEndpoint(endpoint), rememberSession: false }
    try {
      localStorage.setItem('tri-admin.server', browserSettings.apiBaseUrl)
    } catch {
      /* Address remains available for this preview session. */
    }
    return { ...browserSettings }
  },
  vault: {
    load: async (endpoint: string) =>
      native
        ? command<string | null>('load_session', { expectedEndpoint: endpoint })
        : (memory.get(endpoint) ?? null),
    save: async (endpoint: string, token: string) => {
      if (native) await command('save_session', { expectedEndpoint: endpoint, refreshToken: token })
      else memory.set(endpoint, token)
    },
    clear: async (endpoint: string) => {
      if (native) await command('clear_session', { expectedEndpoint: endpoint })
      else memory.delete(endpoint)
    },
  } satisfies SessionVault,
}
