<script setup lang="ts">
import { onBeforeUnmount, reactive, ref } from 'vue'
import schema from './schema.json'
import { Client } from './client.mjs'
type User = { id: string; username: string; canWrite: boolean; dataScope: string }
type Row = {
  id: string
  ownerId: string
  version: number
  createdAt: string
  updatedAt: string
  values: Record<string, unknown>
}
const endpoint = ref('http://127.0.0.1:18089'),
  client = new Client(schema, endpoint.value),
  user = ref<User | null>(null),
  username = ref('admin'),
  password = ref(''),
  error = ref(''),
  loading = ref(false),
  saving = ref(false),
  rows = ref<Row[]>([]),
  page = ref(1),
  pageSize = ref(20),
  total = ref(0),
  search = ref(''),
  open = ref(false),
  editing = ref<Row | null>(null),
  draft = reactive<Record<string, string>>({})
let sequence = 0
function failure(value: unknown) {
  if (!client.user) {
    user.value = null
    rows.value = []
    open.value = false
    total.value = 0
  }
  error.value = value instanceof Error ? value.message : '操作失败'
}
function clear() {
  sequence++
  user.value = null
  rows.value = []
  total.value = 0
  open.value = false
  password.value = ''
  for (const key of Object.keys(draft)) delete draft[key]
}
function switchServer() {
  try {
    client.setEndpoint(endpoint.value)
    clear()
    error.value = ''
  } catch (value) {
    failure(value)
  }
}
async function login() {
  loading.value = true
  error.value = ''
  try {
    user.value = await client.login(username.value, password.value)
    page.value = 1
    await load()
  } catch (value) {
    failure(value)
  } finally {
    password.value = ''
    loading.value = false
  }
}
async function logout() {
  loading.value = true
  try {
    await client.logout()
  } catch (value) {
    failure(value)
  } finally {
    clear()
    loading.value = false
  }
}
async function load() {
  const request = ++sequence
  loading.value = true
  error.value = ''
  try {
    const value = await client.list({
      page: page.value,
      pageSize: pageSize.value,
      search: search.value,
    })
    if (request !== sequence) return
    rows.value = value.data
    total.value = value.meta.total
  } catch (value) {
    if (request === sequence) failure(value)
  } finally {
    if (request === sequence) loading.value = false
  }
}
function query() {
  page.value = 1
  void load()
}
function previous() {
  page.value--
  void load()
}
function next() {
  page.value++
  void load()
}
function edit(row: Row | null) {
  editing.value = row
  error.value = ''
  for (const field of schema.fields) {
    const value = row?.values[field.name]
    draft[field.name] =
      value === null || value === undefined
        ? field.kind === 'boolean' && field.required
          ? 'false'
          : ''
        : Array.isArray(value)
          ? value.join('\n')
          : String(value)
  }
  open.value = true
}
function values() {
  const result: Record<string, unknown> = {}
  for (const field of schema.fields) {
    const value = draft[field.name] ?? ''
    if (!value.trim() && !field.required) {
      result[field.name] = null
      continue
    }
    switch (field.kind) {
      case 'integer': {
        const number = Number(value)
        if (!value.trim() || !Number.isSafeInteger(number))
          throw new Error(field.label + '需要安全范围内的整数')
        result[field.name] = number
        break
      }
      case 'number': {
        const number = Number(value)
        if (!value.trim() || !Number.isFinite(number) || Math.abs(number) > Number.MAX_SAFE_INTEGER)
          throw new Error(field.label + '数值无效')
        result[field.name] = number
        break
      }
      case 'boolean':
        if (!['true', 'false'].includes(value)) throw new Error('请选择' + field.label)
        result[field.name] = value === 'true'
        break
      case 'datetime': {
        const date = new Date(value)
        if (!Number.isFinite(date.getTime())) throw new Error(field.label + '时间无效')
        result[field.name] = date.toISOString()
        break
      }
      case 'textList':
        result[field.name] = value
          .split('\n')
          .map((item) => item.trim())
          .filter(Boolean)
        break
      default:
        if (field.required && !value.trim()) throw new Error('请填写' + field.label)
        result[field.name] = value
    }
  }
  return result
}
async function save() {
  saving.value = true
  error.value = ''
  try {
    const body = values()
    if (editing.value) await client.update(editing.value.id, editing.value.version, body)
    else await client.create(body)
    open.value = false
    await load()
  } catch (value) {
    failure(value)
  } finally {
    saving.value = false
  }
}
async function reload() {
  if (!editing.value) return
  saving.value = true
  try {
    edit(await client.get(editing.value.id))
  } catch (value) {
    failure(value)
  } finally {
    saving.value = false
  }
}
async function remove(row: Row) {
  if (!window.confirm('删除这条记录？')) return
  saving.value = true
  try {
    await client.remove(row.id, row.version)
    await load()
  } catch (value) {
    failure(value)
  } finally {
    saving.value = false
  }
}
function display(value: unknown) {
  return value === null ? '—' : Array.isArray(value) ? value.join(' · ') : String(value)
}
onBeforeUnmount(() => {
  client.clear()
  clear()
})
</script>
<template>
  <main class="workspace">
    <header>
      <div>
        <span class="eyebrow">RESOURCE WORKSPACE</span>
        <h1>{{ schema.label }}</h1>
      </div>
      <div v-if="user" class="identity">
        <span>{{ user.username }} · {{ user.dataScope === 'all' ? '全部数据' : '自身数据' }}</span
        ><button :disabled="loading || saving" @click="logout">退出登录</button>
      </div>
    </header>
    <section class="server">
      <label
        >服务地址<input
          v-model="endpoint"
          aria-label="服务地址"
          :disabled="loading || saving" /></label
      ><button :disabled="loading || saving" @click="switchServer">切换服务</button>
    </section>
    <p v-if="error" role="alert" class="error">{{ error }}</p>
    <form v-if="!user" class="login card" @submit.prevent="login">
      <h2>登录管理工作空间</h2>
      <label
        >账号<input
          v-model="username"
          aria-label="账号"
          autocomplete="username"
          :disabled="loading" /></label
      ><label
        >密码<input
          v-model="password"
          aria-label="密码"
          type="password"
          autocomplete="current-password"
          :disabled="loading" /></label
      ><button class="primary" :disabled="loading">{{ loading ? '正在连接…' : '登录' }}</button>
      <p class="muted">使用在服务端创建的管理员账号。访问凭据只保存在本次页面内存中。</p>
    </form>
    <section v-else class="card">
      <div class="toolbar">
        <input
          v-model="search"
          aria-label="前缀搜索"
          placeholder="按已配置的文本字段前缀搜索"
          :disabled="loading || saving || !schema.fields.some((field) => field.searchable)"
          @keyup.enter="query"
        /><button :disabled="loading || saving" @click="query">查询</button
        ><button :disabled="loading || saving" @click="load">刷新</button
        ><span class="spacer" /><button
          v-if="user.canWrite"
          class="primary"
          :disabled="saving"
          @click="edit(null)"
        >
          新增记录
        </button>
      </div>
      <div class="table">
        <table>
          <thead>
            <tr>
              <th v-for="field in schema.fields" :key="field.name">{{ field.label }}</th>
              <th>版本</th>
              <th v-if="user.canWrite">操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="row in rows" :key="row.id">
              <td v-for="field in schema.fields" :key="field.name">
                {{ display(row.values[field.name]) }}
              </td>
              <td>{{ row.version }}</td>
              <td v-if="user.canWrite" class="actions">
                <button :disabled="saving" @click="edit(row)">编辑</button
                ><button :disabled="saving" @click="remove(row)">删除</button>
              </td>
            </tr>
          </tbody>
        </table>
        <p v-if="!rows.length" class="empty">{{ loading ? '正在读取…' : '暂无记录' }}</p>
      </div>
      <footer>
        <span>共 {{ total }} 条</span
        ><label
          >每页<select v-model.number="pageSize" :disabled="loading || saving" @change="query">
            <option v-for="size in [20, 50, 100]" :key="size" :value="size">{{ size }}</option>
          </select></label
        ><button :disabled="loading || saving || page <= 1" @click="previous">上一页</button
        ><span>第 {{ page }} 页</span
        ><button :disabled="loading || saving || page * pageSize >= total" @click="next">
          下一页
        </button>
      </footer>
    </section>
    <div v-if="open" class="overlay">
      <section
        class="editor card"
        role="dialog"
        aria-modal="true"
        :aria-label="editing ? '编辑记录' : '新增记录'"
      >
        <h2>{{ editing ? '编辑记录' : '新增记录' }}</h2>
        <p v-if="editing" class="muted">
          按版本 {{ editing.version }} 保存；发生冲突时重新读取后再修改。
        </p>
        <form @submit.prevent="save">
          <fieldset :disabled="saving">
            <label v-for="field in schema.fields" :key="field.name"
              >{{ field.label }} {{ field.required ? '*' : ''
              }}<select
                v-if="field.kind === 'boolean'"
                v-model="draft[field.name]"
                :aria-label="field.label"
              >
                <option v-if="!field.required" value="">未设置</option>
                <option value="true">是</option>
                <option value="false">否</option></select
              ><textarea
                v-else-if="field.kind === 'textList'"
                v-model="draft[field.name]"
                :aria-label="field.label"
                placeholder="每行一项"
                rows="3" /><input
                v-else
                v-model="draft[field.name]"
                :aria-label="field.label"
                :type="field.kind === 'integer' || field.kind === 'number' ? 'number' : 'text'"
                :step="field.kind === 'integer' ? '1' : 'any'"
                :maxlength="field.kind === 'text' ? 4096 : undefined"
                :placeholder="field.kind === 'datetime' ? '2026-10-26T00:00:00.000Z' : undefined"
            /></label>
          </fieldset>
          <p v-if="error" role="alert" class="error">{{ error }}</p>
          <div class="toolbar">
            <button v-if="editing" type="button" :disabled="saving" @click="reload">重新读取</button
            ><span class="spacer" /><button type="button" :disabled="saving" @click="open = false">
              取消</button
            ><button class="primary" :disabled="saving">
              {{ saving ? '正在保存…' : '保存记录' }}
            </button>
          </div>
        </form>
      </section>
    </div>
  </main>
</template>
