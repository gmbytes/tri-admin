import { test, expect, type Page } from '@playwright/test'
import { createServer } from 'node:http'
import { mkdir, readFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
let api = '',
  username = ''
async function password() {
  return readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8')
}
test.beforeEach(async ({ page }, info) => {
  api = process.env.TRI_E2E_API!
  username =
    'mcp-ui-' +
    createHash('sha256')
      .update(info.testId + ':' + info.retry)
      .digest('hex')
      .slice(0, 20)
  const created = await page.request.post(api + '/users', {
    headers: { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` },
    data: {
      username,
      displayName: 'MCP UI operator',
      password: await password(),
      roleIds: [process.env.TRI_E2E_OWNER_ROLE],
    },
  })
  expect(created.status()).toBe(201)
  await page.addInitScript((endpoint) => localStorage.setItem('tri-admin.server', endpoint), api)
})
async function login(page: Page, account = username) {
  await page.goto('/#/login')
  await page.getByRole('textbox', { name: '账号', exact: true }).fill(account)
  await page.getByLabel('密码', { exact: true }).fill(await password())
  const reply = page.waitForResponse(
    (response) => response.url() === api + '/auth/login' && response.request().method() === 'POST',
  )
  await page.getByRole('button', { name: '登录', exact: true }).click()
  const body = await (await reply).json()
  await expect(page.locator('.sidebar')).toBeVisible()
  return body.data.accessToken as string
}
async function peer() {
  let effects = 0
  let changed = false,
    listCalls = 0
  const wire: string[] = []
  const server = createServer((request, response) => {
    let raw = ''
    request.on('data', (chunk) => {
      raw += chunk
    })
    request.on('end', () => {
      wire.push(raw)
      const message = JSON.parse(raw)
      if (message.method === 'tools/list') listCalls++
      if (message.method === 'tools/call' && message.params.name === 'hold') {
        effects++
        response.writeHead(200, { 'content-type': 'text/event-stream' })
        response.write(': waiting for fixture cancellation\n\n')
        const timer = setTimeout(() => response.end(), 30_000)
        response.on('close', () => clearTimeout(timer))
        return
      }
      const toolName = message.method === 'tools/call' ? message.params.name : ''
      const special =
        toolName === 'need_input'
          ? { resultType: 'input_required', requestState: 'opaque-fixture-state' }
          : toolName === 'remote_task'
            ? {
                resultType: 'task',
                taskId: 'fixture-task',
                status: 'working',
                createdAt: '2026-10-27T00:00:00Z',
                lastUpdatedAt: '2026-10-27T00:00:00Z',
                ttlMs: 30000,
                pollIntervalMs: 1000,
              }
            : toolName === 'reject'
              ? {
                  resultType: 'complete',
                  content: [{ type: 'text', text: 'Fixture tool rejected the request' }],
                  isError: true,
                }
              : null
      const result =
        special ??
        (message.method === 'server/discover'
          ? {
              resultType: 'complete',
              supportedVersions: ['2026-07-28'],
              capabilities: { tools: {} },
              ttlMs: 0,
              cacheScope: 'private',
            }
          : message.method === 'tools/list'
            ? {
                resultType: 'complete',
                tools: [
                  {
                    name: 'increment',
                    description: 'Increment fixture counter',
                    inputSchema: { type: 'object' },
                  },
                  {
                    name: 'inspect',
                    description: 'Read fixture counter',
                    inputSchema: { type: 'object' },
                  },
                  {
                    name: 'hold',
                    description: 'Hold a response after an external effect',
                    inputSchema: { type: 'object' },
                  },
                  ...['need_input', 'remote_task', 'reject'].map((name) => ({
                    name,
                    inputSchema: { type: 'object' },
                  })),
                  ...(changed
                    ? [
                        {
                          name: 'new_tool',
                          description: 'Newly discovered tool',
                          inputSchema: { type: 'object' },
                        },
                      ]
                    : []),
                ],
                ttlMs: 0,
                cacheScope: 'private',
              }
            : {
                resultType: 'complete',
                content: [],
                structuredContent: {
                  value: message.params.name === 'increment' ? ++effects : effects,
                  rawArgument: raw.match(/18446744073709551615/)?.[0] ?? '',
                },
                isError: false,
              })
      response.writeHead(200, { 'content-type': 'application/json' })
      response.end(JSON.stringify({ jsonrpc: '2.0', id: message.id, result }))
    })
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const address = server.address()
  if (!address || typeof address === 'string') throw Error('No fixture address')
  return {
    url: `http://127.0.0.1:${address.port}/mcp`,
    wire,
    effects: () => effects,
    lists: () => listCalls,
    changeTools: () => {
      changed = true
    },
    close: () =>
      new Promise<void>((resolve, reject) => {
        server.closeAllConnections()
        server.close((error) => (error ? reject(error) : resolve()))
      }),
  }
}
async function seed(
  page: Page,
  token: string,
  remote: { url: string },
  name: string,
  enabled = true,
) {
  const headers = { authorization: `Bearer ${token}` },
    suffix = Date.now().toString(36)
  const profile = await page.request.post(api + '/settings/profiles', {
    headers,
    data: {
      name: 'MCP setup ' + suffix,
      code: 'mcp-setup-' + suffix,
      enabled: true,
      configuration: { kind: 'http', baseUrl: remote.url },
    },
  })
  expect(profile.status()).toBe(201)
  const saved = (await profile.json()).data
  const result = await page.request.post(api + '/mcp/connections', {
    headers,
    data: {
      requestId: crypto.randomUUID(),
      name,
      enabled,
      transport: {
        kind: 'http',
        profileId: saved.id,
        profileVersion: 1,
        allowedTools: [
          'increment',
          'inspect',
          'new_tool',
          'hold',
          'need_input',
          'remote_task',
          'reject',
        ],
      },
    },
  })
  expect(result.status()).toBe(201)
  return (await result.json()).data as { id: string; version: number; transport: unknown }
}
async function completedCall(page: Page, token: string, connectionId: string) {
  const requestId = crypto.randomUUID(),
    headers = { authorization: `Bearer ${token}` }
  const reply = await page.request.post(`${api}/mcp/connections/${connectionId}/calls`, {
    headers,
    data: { requestId, version: 2, name: 'increment', arguments: {} },
  })
  expect(reply.status()).toBe(202)
  const action = (await reply.json()).data as { id: string }
  await expect
    .poll(
      async () =>
        (await (await page.request.get(`${api}/mcp/actions/${action.id}`, { headers })).json()).data
          .status,
    )
    .toBe('succeeded')
  return { ...action, requestId }
}
async function start(page: Page, token: string, id: string, version = 1) {
  const response = await page.request.post(`${api}/mcp/connections/${id}/actions`, {
    headers: { authorization: `Bearer ${token}` },
    data: { requestId: crypto.randomUUID(), version, kind: 'start' },
  })
  expect(response.status()).toBe(202)
  const action = (await response.json()).data
  await expect
    .poll(
      async () =>
        (
          await (
            await page.request.get(`${api}/mcp/actions/${action.id}`, {
              headers: { authorization: `Bearer ${token}` },
            })
          ).json()
        ).data.status,
    )
    .toBe('succeeded')
}
async function workspace(page: Page) {
  await page.locator('nav').getByRole('button', { name: 'MCP连接', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'MCP连接', exact: true })).toBeVisible()
}
async function manage(page: Page, name: string) {
  await page
    .locator('.mcp-connections')
    .getByRole('row')
    .filter({ hasText: name })
    .getByRole('button', { name: '管理', exact: true })
    .click()
  return page.getByRole('dialog', { name: '连接管理', exact: true })
}

test('MCP page manages real HTTP calls, exact arguments and deleted history', async ({ page }) => {
  test.setTimeout(60000)
  const remote = await peer()
  try {
    const token = await login(page),
      name = 'HTTP UI fixture',
      profileName = 'MCP HTTP ' + Date.now().toString(36)
    const profile = await page.request.post(api + '/settings/profiles', {
      headers: { authorization: `Bearer ${token}` },
      data: {
        name: profileName,
        code: 'mcp-ui-' + Date.now().toString(36),
        enabled: true,
        configuration: { kind: 'http', baseUrl: remote.url },
        secret: { kind: 'http', headerName: 'authorization', value: 'Bearer synthetic-ui-peer' },
      },
    })
    expect(profile.status()).toBe(201)
    await page.locator('nav').getByRole('button', { name: 'MCP连接', exact: true }).click()
    await expect(page.getByRole('heading', { name: 'MCP连接', exact: true })).toBeVisible()
    await page.getByRole('button', { name: '新建连接', exact: true }).click()
    let dialog = page.getByRole('dialog', { name: '新建 MCP 连接', exact: true })
    await dialog.getByLabel('连接名称', { exact: true }).fill(name)
    await dialog.getByLabel('传输方式', { exact: true }).press('ArrowDown')
    await page.getByRole('option', { name: 'HTTP', exact: true }).click()
    await dialog.getByLabel('HTTP配置', { exact: true }).press('ArrowDown')
    await page.getByRole('option', { name: new RegExp(profileName) }).click()
    await dialog.getByLabel('批准工具', { exact: true }).fill('increment\ninspect')
    await dialog.getByRole('switch', { name: '允许连接', exact: true }).locator('..').click()
    await expect(dialog.getByRole('switch', { name: '允许连接', exact: true })).toBeChecked()
    const createdConnection = page.waitForResponse(
      (response) =>
        response.url() === api + '/mcp/connections' && response.request().method() === 'POST',
    )
    await dialog.getByRole('button', { name: '保存连接', exact: true }).click()
    const connectionId = (await (await createdConnection).json()).data.id as string
    await expect(dialog).not.toBeVisible()
    expect(remote.wire).toHaveLength(0)
    let row = page.locator('.mcp-connections').getByRole('row').filter({ hasText: name })
    await row.getByRole('button', { name: '管理', exact: true }).click()
    const manager = page.getByRole('dialog', { name: '连接管理', exact: true })
    await manager.getByRole('button', { name: '启动', exact: true }).click()
    await expect(manager.getByText('就绪', { exact: true })).toBeVisible()
    const tool = manager.locator('.mcp-tools').getByRole('row').filter({ hasText: 'increment' })
    await tool.getByRole('button', { name: '调用', exact: true }).click()
    dialog = page.getByRole('dialog', { name: '调用 MCP 工具', exact: true })
    await dialog.getByLabel('参数 JSON', { exact: true }).fill('{"playerId":18446744073709551615}')
    await dialog.getByRole('button', { name: '发送调用', exact: true }).click()
    await expect(dialog).not.toBeVisible()
    await manager.getByRole('tab', { name: '操作历史', exact: true }).click()
    const call = manager.locator('.mcp-history').getByRole('row').filter({ hasText: 'increment' })
    await expect(call.getByText('已返回', { exact: true })).toBeVisible()
    await call.getByRole('button', { name: '结果', exact: true }).click()
    dialog = page.getByRole('dialog', { name: '工具结果', exact: true })
    await expect(dialog.locator('pre')).toContainText('18446744073709551615')
    await dialog.getByRole('button', { name: '关闭结果', exact: true }).click()
    expect(remote.effects()).toBe(1)
    expect(remote.wire.some((raw) => raw.includes('18446744073709551615'))).toBe(true)
    const headers = { authorization: `Bearer ${token}` }
    const original = (
      await (await page.request.get(`${api}/mcp/connections/${connectionId}`, { headers })).json()
    ).data.runtime.id
    const restarted = page.waitForResponse(
      (response) =>
        response.url() === `${api}/mcp/connections/${connectionId}/actions` &&
        response.request().method() === 'POST',
    )
    await manager.getByRole('button', { name: '重启', exact: true }).click()
    expect((await restarted).status()).toBe(202)
    await expect(manager.getByText('连接版本 3', { exact: true })).toBeVisible()
    await expect(manager.getByText('就绪', { exact: true })).toBeVisible()
    const current = (
      await (await page.request.get(`${api}/mcp/connections/${connectionId}`, { headers })).json()
    ).data
    expect(current.runtime.id).not.toBe(original)
    expect(remote.wire.filter((raw) => JSON.parse(raw).method === 'server/discover')).toHaveLength(
      2,
    )
    expect(remote.effects()).toBe(1)
    await manager.getByRole('button', { name: '停止', exact: true }).click()
    await expect(manager.getByText('已停止', { exact: true })).toBeVisible()
    await manager.getByRole('button', { name: '关闭管理', exact: true }).click()
    await row.getByRole('button', { name: '删除', exact: true }).click()
    await page.getByRole('button', { name: '删除连接', exact: true }).click()
    await expect(
      page.locator('.mcp-connections').getByRole('row').filter({ hasText: name }),
    ).toHaveCount(0)
    await page.getByRole('tab', { name: '已删除', exact: true }).click()
    row = page.locator('.mcp-connections').getByRole('row').filter({ hasText: name })
    await row.getByRole('button', { name: '管理', exact: true }).click()
    await expect(manager.getByRole('button', { name: '启动', exact: true })).toHaveCount(0)
    await manager.getByRole('tab', { name: '操作历史', exact: true }).click()
    await expect(
      manager.locator('.mcp-history').getByRole('row').filter({ hasText: 'increment' }),
    ).toHaveCount(1)
  } finally {
    await remote.close()
  }
})

test('lost call responses recover the original UUID after reload without persisting arguments', async ({
  page,
}) => {
  test.setTimeout(60000)
  const remote = await peer()
  try {
    const token = await login(page),
      name = 'Recover HTTP'
    const connection = await seed(page, token, remote, name)
    await start(page, token, connection.id)
    await workspace(page)
    let manager = await manage(page, name)
    let writes = 0
    await page.route('**/api/v1/mcp/connections/*/calls', async (route) => {
      writes++
      const reply = await route.fetch()
      expect(reply.status()).toBe(202)
      await route.abort('connectionfailed')
    })
    await manager
      .locator('.mcp-tools')
      .getByRole('row')
      .filter({ hasText: 'increment' })
      .getByRole('button', { name: '调用', exact: true })
      .click()
    const dialog = page.getByRole('dialog', { name: '调用 MCP 工具', exact: true })
    await dialog
      .getByLabel('参数 JSON', { exact: true })
      .fill('{"note":"mcp-arguments-must-not-persist"}')
    await dialog.getByRole('button', { name: '发送调用', exact: true }).click()
    await expect(
      dialog.getByText('无法连接服务器，请检查地址和网络', { exact: true }),
    ).toBeVisible()
    await expect.poll(remote.effects).toBe(1)
    const raw = await page.evaluate(() =>
      Object.entries(localStorage)
        .filter(([key]) => key.startsWith('tri-admin.mcp.v1:'))
        .map(([, value]) => value)
        .join(''),
    )
    expect(raw).toContain('requestId')
    expect(raw).not.toContain('mcp-arguments-must-not-persist')
    expect(raw).not.toContain('arguments')
    await page.unrouteAll({ behavior: 'wait' })
    await page.reload()
    await login(page)
    await workspace(page)
    await expect(page.getByText('待确认 MCP 操作', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: '恢复查询', exact: true }).click()
    await expect(page.getByText('待确认 MCP 操作', { exact: true })).toHaveCount(0)
    manager = page.getByRole('dialog', { name: '连接管理', exact: true })
    await expect(
      manager
        .locator('.mcp-history')
        .getByRole('row')
        .filter({ hasText: 'increment' })
        .getByText('已返回', { exact: true }),
    ).toBeVisible()
    expect(writes).toBe(1)
    expect(remote.effects()).toBe(1)
  } finally {
    await remote.close()
  }
})

test('readonly users see scoped metadata but no lifecycle, call or private result controls', async ({
  page,
  browser,
}) => {
  test.setTimeout(60000)
  const remote = await peer()
  const context = await browser.newContext()
  try {
    const token = await login(page),
      name = 'Readonly HTTP',
      headers = { authorization: `Bearer ${token}` }
    const connection = await seed(page, token, remote, name)
    await start(page, token, connection.id)
    const role = await page.request.post(api + '/roles', {
      headers,
      data: {
        name: 'MCP reader',
        slug: 'mcp-reader-' + Date.now().toString(36),
        permissions: ['mcp.read'],
        dataScope: 'all',
      },
    })
    expect(role.status()).toBe(201)
    const reader = 'mcp-read-' + Date.now().toString(36)
    const user = await page.request.post(api + '/users', {
      headers,
      data: {
        username: reader,
        displayName: 'Readonly MCP',
        password: await password(),
        roleIds: [(await role.json()).data.id],
      },
    })
    expect(user.status()).toBe(201)
    const operator = await context.newPage()
    await operator.addInitScript(
      (endpoint) => localStorage.setItem('tri-admin.server', endpoint),
      api,
    )
    await login(operator, reader)
    await workspace(operator)
    await expect(operator.getByRole('button', { name: '新建连接', exact: true })).toHaveCount(0)
    const manager = await manage(operator, name)
    for (const button of ['启动', '停止', '重启', '刷新工具'])
      await expect(manager.getByRole('button', { name: button, exact: true })).toHaveCount(0)
    await expect(
      manager.locator('.mcp-tools').getByRole('row').filter({ hasText: 'increment' }),
    ).toHaveCount(1)
    await expect(manager.getByRole('button', { name: '调用', exact: true })).toHaveCount(0)
    await manager.getByRole('tab', { name: '操作历史', exact: true }).click()
    await expect(manager.getByRole('button', { name: '结果', exact: true })).toHaveCount(0)
    expect(remote.effects()).toBe(0)
  } finally {
    await context.close()
    await remote.close()
  }
})

test('version conflicts keep the draft and require an explicit current-record reload', async ({
  page,
}) => {
  test.setTimeout(60000)
  const remote = await peer()
  try {
    const token = await login(page),
      name = 'Conflict HTTP',
      headers = { authorization: `Bearer ${token}` }
    const connection = await seed(page, token, remote, name, false)
    await workspace(page)
    await page
      .locator('.mcp-connections')
      .getByRole('row')
      .filter({ hasText: name })
      .getByRole('button', { name: '编辑', exact: true })
      .click()
    const dialog = page.getByRole('dialog', { name: '编辑 MCP 连接', exact: true })
    await dialog.getByLabel('连接名称', { exact: true }).fill('Local draft')
    const changed = await page.request.patch(`${api}/mcp/connections/${connection.id}`, {
      headers,
      data: { version: 1, name: 'Server changed', enabled: false, transport: connection.transport },
    })
    expect(changed.status()).toBe(200)
    let writes = 0
    await page.route(`**/api/v1/mcp/connections/${connection.id}`, (route) => {
      if (route.request().method() === 'PATCH') writes++
      return route.continue()
    })
    await dialog.getByRole('button', { name: '保存连接', exact: true }).click()
    await expect(dialog.getByText('连接版本已改变', { exact: true })).toBeVisible()
    await expect(dialog.getByLabel('连接名称', { exact: true })).toHaveValue('Local draft')
    expect(writes).toBe(1)
    await dialog.getByRole('button', { name: '重新载入记录', exact: true }).click()
    await expect(dialog.getByLabel('连接名称', { exact: true })).toHaveValue('Server changed')
    await dialog.getByLabel('连接名称', { exact: true }).fill('Reviewed change')
    await dialog.getByRole('button', { name: '保存连接', exact: true }).click()
    await expect(dialog).not.toBeVisible()
    expect(writes).toBe(2)
    await expect(page.locator('.mcp-connections')).toContainText('Reviewed change')
  } finally {
    await remote.close()
  }
})

test('tool refresh is explicit and preserves the running instance', async ({ page }) => {
  test.setTimeout(60000)
  const remote = await peer()
  try {
    const token = await login(page),
      name = 'Refresh HTTP'
    const connection = await seed(page, token, remote, name)
    await start(page, token, connection.id)
    await workspace(page)
    let manager = await manage(page, name)
    await expect(
      manager.locator('.mcp-tools').getByRole('row').filter({ hasText: 'new_tool' }),
    ).toHaveCount(0)
    expect(remote.lists()).toBe(1)
    remote.changeTools()
    await manager.getByRole('button', { name: '关闭管理', exact: true }).click()
    await page.getByRole('button', { name: '刷新列表', exact: true }).click()
    manager = await manage(page, name)
    await expect(
      manager.locator('.mcp-tools').getByRole('row').filter({ hasText: 'new_tool' }),
    ).toHaveCount(0)
    expect(remote.lists()).toBe(1)
    await manager.getByRole('button', { name: '刷新工具', exact: true }).click()
    await expect(
      manager.locator('.mcp-tools').getByRole('row').filter({ hasText: 'new_tool' }),
    ).toHaveCount(1)
    expect(remote.lists()).toBe(2)
    const current = await page.request.get(`${api}/mcp/connections/${connection.id}`, {
      headers: { authorization: `Bearer ${token}` },
    })
    expect((await current.json()).data.version).toBe(2)
  } finally {
    await remote.close()
  }
})

test('cancelling an already sent call preserves unknown effects and never resends it', async ({
  page,
}) => {
  test.setTimeout(60_000)
  const remote = await peer()
  try {
    const token = await login(page),
      name = 'Cancel HTTP'
    const connection = await seed(page, token, remote, name)
    await start(page, token, connection.id)
    await workspace(page)
    const manager = await manage(page, name)
    await manager
      .locator('.mcp-tools')
      .getByRole('row')
      .filter({ hasText: 'hold' })
      .getByRole('button', { name: '调用', exact: true })
      .click()
    const dialog = page.getByRole('dialog', { name: '调用 MCP 工具', exact: true })
    await dialog.getByRole('button', { name: '发送调用', exact: true }).click()
    await expect(dialog).not.toBeVisible()
    await expect.poll(remote.effects).toBe(1)
    const call = manager.locator('.mcp-history').getByRole('row').filter({ hasText: 'hold' })
    await call.getByRole('button', { name: '取消', exact: true }).click()
    await expect(call.getByText('效果未确认', { exact: true })).toBeVisible()
    await expect(call.getByRole('button', { name: '结果', exact: true })).toHaveCount(0)
    await expect(page.getByText('待确认 MCP 操作', { exact: true })).toHaveCount(0)
    expect(remote.effects()).toBe(1)
    expect(remote.wire.filter((raw) => JSON.parse(raw).method === 'tools/call')).toHaveLength(1)
  } finally {
    await remote.close()
  }
})

test('corrupt or expired result responses clear the previous private payload', async ({ page }) => {
  test.setTimeout(60_000)
  const remote = await peer()
  try {
    const token = await login(page),
      name = 'Result verification HTTP'
    const connection = await seed(page, token, remote, name)
    await start(page, token, connection.id)
    const action = await completedCall(page, token, connection.id)
    await workspace(page)
    const manager = await manage(page, name)
    await manager.getByRole('tab', { name: '操作历史', exact: true }).click()
    const row = manager
      .locator('.mcp-history')
      .getByRole('row')
      .filter({ hasText: action.requestId })
    const result = page.getByRole('dialog', { name: '工具结果', exact: true })
    await row.getByRole('button', { name: '结果', exact: true }).click()
    await expect(result.locator('pre')).toContainText('"value":1')
    await result.getByRole('button', { name: '关闭结果', exact: true }).click()
    await page.route(`**/api/v1/mcp/actions/${action.id}/result`, async (route) => {
      const reply = await route.fetch(),
        body = await reply.json()
      body.data.sha256 = '0'.repeat(64)
      await route.fulfill({ response: reply, json: body })
    })
    await row.getByRole('button', { name: '结果', exact: true }).click()
    await expect(result.getByText('MCP 响应或恢复记录无效', { exact: true })).toBeVisible()
    await expect(result.locator('pre')).toHaveCount(0)
    await result.getByRole('button', { name: '关闭结果', exact: true }).click()
    await page.unrouteAll({ behavior: 'wait' })
    await page.route(`**/api/v1/mcp/actions/${action.id}/result`, (route) =>
      route.fulfill({
        status: 404,
        contentType: 'application/json',
        body: JSON.stringify({ error: { code: 'NOT_FOUND', message: '结果已过期' } }),
      }),
    )
    await row.getByRole('button', { name: '结果', exact: true }).click()
    await expect(result.getByText('结果已过期', { exact: true })).toBeVisible()
    await expect(result.locator('pre')).toHaveCount(0)
    expect(remote.effects()).toBe(1)
  } finally {
    await remote.close()
  }
})

test('late private results cannot replace a subsequently opened result', async ({ page }) => {
  test.setTimeout(60_000)
  const remote = await peer()
  let release = () => {}
  try {
    const token = await login(page),
      name = 'Late result HTTP'
    const connection = await seed(page, token, remote, name)
    await start(page, token, connection.id)
    const first = await completedCall(page, token, connection.id)
    const second = await completedCall(page, token, connection.id)
    await workspace(page)
    const manager = await manage(page, name)
    await manager.getByRole('tab', { name: '操作历史', exact: true }).click()
    const delayed = new Promise<void>((resolve) => {
      release = resolve
    })
    let fetched = false
    const finished = page.waitForResponse(`${api}/mcp/actions/${first.id}/result`)
    await page.route(`**/api/v1/mcp/actions/${first.id}/result`, async (route) => {
      const response = await route.fetch()
      fetched = true
      await delayed
      await route.fulfill({ response })
    })
    await manager
      .locator('.mcp-history')
      .getByRole('row')
      .filter({ hasText: first.requestId })
      .getByRole('button', { name: '结果', exact: true })
      .click()
    await expect.poll(() => fetched).toBe(true)
    const result = page.getByRole('dialog', { name: '工具结果', exact: true })
    await result.getByRole('button', { name: '关闭结果', exact: true }).click()
    await manager
      .locator('.mcp-history')
      .getByRole('row')
      .filter({ hasText: second.requestId })
      .getByRole('button', { name: '结果', exact: true })
      .click()
    await expect(result.locator('pre')).toContainText('"value":2')
    release()
    await finished
    await expect(result.locator('pre')).toContainText('"value":2')
    await expect(result.locator('pre')).not.toContainText('"value":1')
  } finally {
    release()
    await remote.close()
  }
})

test('input requests, remote tasks and tool errors are displayed without claiming business success', async ({
  page,
}) => {
  test.setTimeout(60_000)
  const remote = await peer()
  try {
    const token = await login(page),
      name = 'Response kinds HTTP'
    const connection = await seed(page, token, remote, name)
    await start(page, token, connection.id)
    await workspace(page)
    const manager = await manage(page, name)
    for (const [tool, historyLabel, resultLabel] of [
      ['need_input', '需要后续输入', '需要后续输入'],
      ['remote_task', '收到远端任务', '收到远端任务'],
      ['reject', '失败', '工具返回错误'],
    ]) {
      await manager.getByRole('tab', { name: '工具清单', exact: true }).click()
      await manager
        .locator('.mcp-tools')
        .getByRole('row')
        .filter({ hasText: tool })
        .getByRole('button', { name: '调用', exact: true })
        .click()
      const call = page.getByRole('dialog', { name: '调用 MCP 工具', exact: true })
      await call.getByRole('button', { name: '发送调用', exact: true }).click()
      await expect(call).not.toBeVisible()
      const row = manager.locator('.mcp-history').getByRole('row').filter({ hasText: tool })
      await expect(row.getByText(historyLabel, { exact: true })).toBeVisible()
      await row.getByRole('button', { name: '结果', exact: true }).click()
      const result = page.getByRole('dialog', { name: '工具结果', exact: true })
      await expect(result.getByText(resultLabel, { exact: true })).toBeVisible()
      await result.getByRole('button', { name: '关闭结果', exact: true }).click()
    }
    expect(remote.effects()).toBe(0)
    expect(remote.wire.filter((raw) => JSON.parse(raw).method === 'tools/call')).toHaveLength(3)
    expect(
      remote.wire.some((raw) => ['tasks/get', 'tasks/cancel'].includes(JSON.parse(raw).method)),
    ).toBe(false)
  } finally {
    await remote.close()
  }
})

test('MCP management and private results fit desktop and narrow windows', async ({ page }) => {
  test.setTimeout(60_000)
  const remote = await peer()
  try {
    await page.setViewportSize({ width: 1440, height: 1000 })
    const token = await login(page),
      name = 'MCP 受控连接演示'
    const connection = await seed(page, token, remote, name)
    await start(page, token, connection.id)
    const action = await completedCall(page, token, connection.id)
    await workspace(page)
    const manager = await manage(page, name)
    await expect(manager.getByText('就绪', { exact: true })).toBeVisible()
    const directory = '.superpowers/sdd/2026-10-24-tri-admin-greenfield/mcp-ui-visual'
    await mkdir(directory, { recursive: true })
    await page.screenshot({ path: `${directory}/desktop-manager.png`, animations: 'disabled' })
    await manager.getByRole('tab', { name: '操作历史', exact: true }).click()
    await manager
      .locator('.mcp-history')
      .getByRole('row')
      .filter({ hasText: action.requestId })
      .getByRole('button', { name: '结果', exact: true })
      .click()
    const result = page.getByRole('dialog', { name: '工具结果', exact: true })
    await expect(result.locator('pre')).toContainText('"value":1')
    await page.screenshot({ path: `${directory}/desktop-result.png`, animations: 'disabled' })
    await page.setViewportSize({ width: 760, height: 900 })
    await expect
      .poll(async () => (await result.locator('.el-dialog').boundingBox())?.width ?? 1000)
      .toBeLessThanOrEqual(736)
    await expect(result.getByRole('button', { name: '关闭结果', exact: true })).toBeInViewport()
    await expect
      .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
      .toBeLessThanOrEqual(760)
    await page.screenshot({ path: `${directory}/narrow-result.png`, animations: 'disabled' })
    await result.getByRole('button', { name: '关闭结果', exact: true }).click()
    await expect(result).not.toBeVisible()
    await expect(manager.getByRole('button', { name: '关闭管理', exact: true })).toBeInViewport()
    await page.screenshot({ path: `${directory}/narrow-manager.png`, animations: 'disabled' })
  } finally {
    await remote.close()
  }
})

test('revoking invoke permission rejects a stale page before any upstream effect', async ({
  page,
  browser,
}) => {
  test.setTimeout(60_000)
  const remote = await peer(),
    context = await browser.newContext()
  try {
    const token = await login(page),
      name = 'Current authority HTTP'
    const connection = await seed(page, token, remote, name)
    await start(page, token, connection.id)
    const headers = { authorization: `Bearer ${token}` },
      suffix = Date.now().toString(36)
    const roleReply = await page.request.post(api + '/roles', {
      headers,
      data: {
        name: 'MCP caller',
        slug: 'mcp-caller-' + suffix,
        permissions: ['mcp.read', 'mcp.invoke'],
        dataScope: 'all',
      },
    })
    expect(roleReply.status()).toBe(201)
    const role = (await roleReply.json()).data,
      account = 'mcp-caller-' + suffix
    expect(
      (
        await page.request.post(api + '/users', {
          headers,
          data: {
            username: account,
            displayName: 'MCP caller',
            password: await password(),
            roleIds: [role.id],
          },
        })
      ).status(),
    ).toBe(201)
    const operator = await context.newPage()
    await operator.addInitScript(
      (endpoint) => localStorage.setItem('tri-admin.server', endpoint),
      api,
    )
    await login(operator, account)
    await workspace(operator)
    const manager = await manage(operator, name)
    await manager
      .locator('.mcp-tools')
      .getByRole('row')
      .filter({ hasText: 'increment' })
      .getByRole('button', { name: '调用', exact: true })
      .click()
    const dialog = operator.getByRole('dialog', { name: '调用 MCP 工具', exact: true })
    const changed = await page.request.patch(`${api}/roles/${role.id}`, {
      headers,
      data: { version: 1, permissions: ['mcp.read'] },
    })
    expect(changed.status()).toBe(200)
    const rejected = operator.waitForResponse(
      (response) =>
        response.url() === `${api}/mcp/connections/${connection.id}/calls` &&
        response.request().method() === 'POST',
    )
    await dialog.getByRole('button', { name: '发送调用', exact: true }).click()
    const response = await rejected
    expect(response.status()).toBe(403)
    await expect(
      dialog.getByText((await response.json()).error.message, { exact: true }),
    ).toBeVisible()
    expect(remote.effects()).toBe(0)
    expect(remote.wire.some((raw) => JSON.parse(raw).method === 'tools/call')).toBe(false)
    expect(
      await operator.evaluate(() =>
        Object.entries(localStorage).filter(([key]) => key.startsWith('tri-admin.mcp.v1:')),
      ),
    ).toEqual([])
  } finally {
    await context.close()
    await remote.close()
  }
})

test('a slow history response does not accumulate overlapping background polls', async ({
  page,
}) => {
  test.setTimeout(60_000)
  const remote = await peer()
  let release = () => {},
    activeReads = 0
  try {
    const token = await login(page),
      name = 'Slow history HTTP'
    const connection = await seed(page, token, remote, name)
    await start(page, token, connection.id)
    await workspace(page)
    const delayed = new Promise<void>((resolve) => {
      release = resolve
    })
    let historyReads = 0,
      thirdRead = () => {}
    const extra = new Promise<void>((resolve) => {
      thirdRead = resolve
    })
    await page.route(new RegExp(`/mcp/connections/${connection.id}/actions\\?`), async (route) => {
      historyReads++
      activeReads++
      if (historyReads === 3) thirdRead()
      try {
        const response = await route.fetch()
        await delayed
        await route.fulfill({ response })
      } finally {
        activeReads--
      }
    })
    const manager = await manage(page, name)
    await expect.poll(() => historyReads).toBe(2)
    const accumulated = await new Promise<boolean>((resolve) => {
      const deadline = setTimeout(() => resolve(false), 3000)
      extra.then(() => {
        clearTimeout(deadline)
        resolve(true)
      })
    })
    release()
    await expect.poll(() => activeReads).toBe(0)
    expect(accumulated).toBe(false)
    await manager.getByRole('tab', { name: '操作历史', exact: true }).click()
    await expect(manager.locator('.mcp-history').getByText('已完成', { exact: true })).toBeVisible()
  } finally {
    release()
    await expect.poll(() => activeReads).toBe(0)
    await page.unrouteAll({ behavior: 'wait' })
    await remote.close()
  }
})
