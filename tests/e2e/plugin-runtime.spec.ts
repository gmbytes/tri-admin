import { test, expect, type Page } from '@playwright/test'
import { mkdir, readFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { pluginArchive } from './plugin-fixture'
let api = '',
  username = ''
async function password() {
  return readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8')
}
test.beforeEach(async ({ page }) => {
  api = process.env.TRI_E2E_API!
  username = 'plugin-' + randomUUID().replaceAll('-', '')
  const created = await page.request.post(api + '/users', {
    headers: { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` },
    data: {
      username,
      displayName: '插件运行验证',
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
  const response = page.waitForResponse(
    (value) => value.url() === api + '/auth/login' && value.request().method() === 'POST',
  )
  await page.getByRole('button', { name: '登录', exact: true }).click()
  const signed = await (await response).json()
  await expect(page.locator('.sidebar')).toBeVisible()
  return signed.data.accessToken as string
}
async function install(
  page: Page,
  token: string,
  name: string,
  tools: unknown[] = [],
  mcp: unknown[] = [],
) {
  const bytes = pluginArchive({
    formatVersion: 1,
    name,
    version: '1.0.0',
    description: 'Plugin runtime fixture',
    skills: [],
    tools,
    mcp,
  })
  const result = await page.request.post(api + '/extensions', {
    headers: { authorization: `Bearer ${token}` },
    data: {
      requestId: randomUUID(),
      source: { kind: 'archive', contentBase64: bytes.toString('base64') },
    },
  })
  expect(result.status()).toBe(201)
  return (await result.json()).data as { id: string; revisionId: string; version: number }
}

test('plugin source picker pages approved revisions and manages a real owned process', async ({
  page,
}) => {
  test.setTimeout(90000)
  const token = await login(page),
    headers = { authorization: `Bearer ${token}` },
    prefix = 'source-' + Date.now().toString(36)
  let selected: { id: string; revisionId: string; version: number } | undefined
  for (let index = 0; index < 21; index++) {
    const row = await install(
      page,
      token,
      `${prefix}-${String(index).padStart(2, '0')}`,
      [],
      [
        {
          id: 'records',
          label: `记录来源 ${index}`,
          capabilityId: 'e2e-records',
          tools: ['increment', 'inspect'],
        },
      ],
    )
    expect(
      (
        await page.request.patch(`${api}/extensions/${row.id}`, {
          headers,
          data: { version: 1, enabled: true },
        })
      ).status(),
    ).toBe(200)
    if (index === 20) selected = row
  }
  await page.locator('nav').getByRole('button', { name: 'MCP连接', exact: true }).click()
  await page.getByRole('button', { name: '新建连接', exact: true }).click()
  const editor = page.getByRole('dialog', { name: '新建 MCP 连接', exact: true })
  await editor.getByLabel('连接名称', { exact: true }).fill('插件受控进程')
  await editor.getByLabel('传输方式', { exact: true }).press('ArrowDown')
  await page.getByRole('option', { name: '插件 MCP', exact: true }).click()
  await editor.getByRole('button', { name: '选择插件来源', exact: true }).click()
  const picker = page.getByRole('dialog', { name: '选择插件来源', exact: true })
  await picker.getByPlaceholder('按插件名称前缀搜索').fill(prefix)
  await picker.getByRole('button', { name: '搜索来源', exact: true }).click()
  await expect(picker.getByText('共 21 条', { exact: true })).toBeVisible()
  const box = await picker.locator('.el-dialog').boundingBox()
  expect(box!.y + box!.height).toBeLessThanOrEqual(page.viewportSize()!.height - 8)
  await expect(picker.getByRole('button', { name: '关闭选择', exact: true })).toBeInViewport()
  const visual = '.superpowers/sdd/2026-10-24-tri-admin-greenfield/plugin-ui-visual'
  await mkdir(visual, { recursive: true })
  await page.screenshot({ path: visual + '/picker-desktop.png', animations: 'disabled' })
  await page.setViewportSize({ width: 760, height: 900 })
  await expect(picker.getByRole('button', { name: '关闭选择', exact: true })).toBeInViewport()
  await page.screenshot({ path: visual + '/picker-narrow.png', animations: 'disabled' })
  await page.setViewportSize({ width: 1280, height: 720 })
  await picker.getByRole('button', { name: '下一页', exact: true }).click()
  await picker
    .getByRole('row')
    .filter({ hasText: `${prefix}-20` })
    .getByRole('button', { name: '使用来源', exact: true })
    .click()
  await expect(picker).not.toBeVisible()
  await expect(editor).toContainText(`${prefix}-20`)
  await editor.getByRole('switch', { name: '允许连接', exact: true }).locator('..').click()
  const created = page.waitForResponse(
    (response) =>
      response.url() === api + '/mcp/connections' && response.request().method() === 'POST',
  )
  await editor.getByRole('button', { name: '保存连接', exact: true }).click()
  const connection = (await (await created).json()).data
  expect(connection.transport).toEqual({
    kind: 'plugin',
    extensionId: selected!.id,
    revisionId: selected!.revisionId,
    contributionId: 'records',
  })
  await page
    .locator('.mcp-connections')
    .getByRole('row')
    .filter({ hasText: '插件受控进程' })
    .getByRole('button', { name: '管理', exact: true })
    .click()
  const manager = page.getByRole('dialog', { name: '连接管理', exact: true })
  await manager.getByRole('button', { name: '启动', exact: true }).click()
  await expect(manager.getByText('就绪', { exact: true })).toBeVisible()
  await expect(manager.getByText(/PID \d+/)).toBeVisible()
  await page.screenshot({ path: visual + '/process-manager.png', animations: 'disabled' })
  await manager
    .locator('.mcp-tools')
    .getByRole('row')
    .filter({ hasText: 'increment' })
    .getByRole('button', { name: '调用', exact: true })
    .click()
  const calling = page.getByRole('dialog', { name: '调用 MCP 工具', exact: true })
  await calling.getByRole('button', { name: '发送调用', exact: true }).click()
  await expect(calling).not.toBeVisible()
  const call = manager.locator('.mcp-history').getByRole('row').filter({ hasText: 'increment' })
  await expect(call.getByText('已返回', { exact: true })).toBeVisible()
  await call.getByRole('button', { name: '结果', exact: true }).click()
  const result = page.getByRole('dialog', { name: '工具结果', exact: true })
  await expect(result.locator('pre')).toContainText('"value":1')
  await result.getByRole('button', { name: '关闭结果', exact: true }).click()
  await expect(result).not.toBeVisible()
  const beforeRestart = (
    await (await page.request.get(`${api}/mcp/connections/${connection.id}`, { headers })).json()
  ).data.runtime
  await manager.getByRole('button', { name: '重启', exact: true }).click()
  await expect
    .poll(async () => {
      const row = (
        await (
          await page.request.get(`${api}/mcp/connections/${connection.id}`, { headers })
        ).json()
      ).data
      return (
        row.runtime?.state === 'running' &&
        row.runtime.id !== beforeRestart.id &&
        row.runtime.pid !== beforeRestart.pid
      )
    })
    .toBe(true)
  await expect(manager.getByText('就绪', { exact: true })).toBeVisible()
  expect(
    (
      await page.request.patch(`${api}/extensions/${selected!.id}`, {
        headers,
        data: { version: 2, enabled: false },
      })
    ).status(),
  ).toBe(200)
  await expect(manager.getByText('已停止', { exact: true })).toBeVisible({ timeout: 10000 })
  await expect(
    manager.getByText('绑定已变更；确认旧资源释放后重新绑定配置。', { exact: true }),
  ).toBeVisible()
  expect(
    (
      await page.request.patch(`${api}/extensions/${selected!.id}`, {
        headers,
        data: { version: 3, enabled: true },
      })
    ).status(),
  ).toBe(200)
  await manager.getByRole('button', { name: '关闭管理', exact: true }).click()
  const saved = page
    .locator('.mcp-connections')
    .getByRole('row')
    .filter({ hasText: '插件受控进程' })
  await saved.getByRole('button', { name: '编辑', exact: true }).click()
  const rebinding = page.getByRole('dialog', { name: '编辑 MCP 连接', exact: true })
  await rebinding.getByRole('button', { name: '保存连接', exact: true }).click()
  await expect(rebinding.getByText('请重新选择当前插件来源', { exact: true })).toBeVisible()
  await rebinding.getByRole('button', { name: '选择插件来源', exact: true }).click()
  await picker.getByPlaceholder('按插件名称前缀搜索').fill(`${prefix}-20`)
  await picker.getByRole('button', { name: '搜索来源', exact: true }).click()
  await picker
    .getByRole('row')
    .filter({ hasText: `${prefix}-20` })
    .getByRole('button', { name: '使用来源', exact: true })
    .click()
  await expect(rebinding).toContainText('版本 4')
  await rebinding.getByRole('button', { name: '保存连接', exact: true }).click()
  await expect(rebinding).not.toBeVisible()
  await saved.getByRole('button', { name: '管理', exact: true }).click()
  await manager.getByRole('button', { name: '启动', exact: true }).click()
  await expect(manager.getByText('就绪', { exact: true })).toBeVisible()
  await manager.getByRole('button', { name: '停止', exact: true }).click()
  await expect(manager.getByText('已停止', { exact: true })).toBeVisible()
})

test('plugin business bindings use real HTTP MCP calls and recover a lost UUID without a second effect', async ({
  page,
}) => {
  test.setTimeout(90000)
  const token = await login(page),
    headers = { authorization: `Bearer ${token}` },
    name = 'business-' + Date.now().toString(36),
    key = 'plugin.ui.' + Date.now().toString(36)
  const tools = [
    {
      name: 'create-record',
      description: 'Create a managed parameter',
      operation: 'parameters.create',
      inputSchema: {
        type: 'object',
        properties: {
          caption: { type: 'string' },
          recordKey: { type: 'string' },
          kind: { enum: ['string'] },
          payload: { type: 'string' },
        },
        required: ['caption', 'recordKey', 'kind', 'payload'],
        additionalProperties: false,
      },
      mappings: { name: 'caption', key: 'recordKey', valueType: 'kind', value: 'payload' },
    },
    {
      name: 'search-records',
      description: 'Search managed parameters',
      operation: 'parameters.list',
      inputSchema: {
        type: 'object',
        properties: { prefix: { type: 'string' } },
        additionalProperties: false,
      },
      mappings: { search: 'prefix' },
    },
  ]
  const extension = await install(page, token, name, tools)
  await page.locator('nav').getByRole('button', { name: 'Skills 与插件', exact: true }).click()
  const row = page.locator('.extensions-table').getByRole('row').filter({ hasText: name })
  await row.getByRole('button', { name: '管理文件', exact: true }).click()
  const library = page.getByRole('dialog', { name: '扩展文件与历史', exact: true })
  await library.getByRole('button', { name: '贡献能力', exact: true }).click()
  const toolName = `plugin.${extension.id}.${extension.revisionId}.create-record`
  await expect(library).toContainText(toolName)
  const visual = '.superpowers/sdd/2026-10-24-tri-admin-greenfield/plugin-ui-visual'
  await mkdir(visual, { recursive: true })
  await page.screenshot({ path: visual + '/contributions.png', animations: 'disabled' })
  await library.getByRole('button', { name: '关闭此对话框', exact: true }).click()
  await expect(library).not.toBeVisible()
  await row.getByRole('button', { name: '启用', exact: true }).click()
  await expect(row.getByRole('button', { name: '停用', exact: true })).toBeVisible()
  const issued = await page.request.post(api + '/api-tokens', {
    headers,
    data: {
      requestId: randomUUID(),
      name: 'Plugin HTTP fixture',
      scopes: ['mcp.read', 'mcp.invoke', 'skills.read', 'parameters.read', 'parameters.write'],
    },
  })
  expect(issued.status()).toBe(201)
  const credential = (await issued.json()).data.secret
  const profile = await page.request.post(api + '/settings/profiles', {
    headers,
    data: {
      name: 'Plugin business MCP',
      code: 'plugin-http-' + Date.now().toString(36),
      enabled: true,
      configuration: { kind: 'http', baseUrl: api + '/mcp/server' },
      secret: { kind: 'http', headerName: 'authorization', value: `Bearer ${credential}` },
    },
  })
  expect(profile.status()).toBe(201)
  const created = await page.request.post(api + '/mcp/connections', {
    headers,
    data: {
      requestId: randomUUID(),
      name: '插件业务连接',
      enabled: true,
      transport: {
        kind: 'http',
        profileId: (await profile.json()).data.id,
        profileVersion: 1,
        allowedTools: [toolName],
      },
    },
  })
  expect(created.status()).toBe(201)
  const connection = (await created.json()).data
  await page.locator('nav').getByRole('button', { name: 'MCP连接', exact: true }).click()
  await page
    .locator('.mcp-connections')
    .getByRole('row')
    .filter({ hasText: '插件业务连接' })
    .getByRole('button', { name: '管理', exact: true })
    .click()
  let manager = page.getByRole('dialog', { name: '连接管理', exact: true })
  await manager.getByRole('button', { name: '启动', exact: true }).click()
  await expect(manager.getByText('就绪', { exact: true })).toBeVisible()
  await expect(manager.getByText(/PID \d+/)).toHaveCount(0)
  let writes = 0
  await page.route(`**/api/v1/mcp/connections/${connection.id}/calls`, async (route) => {
    writes++
    const reply = await route.fetch()
    expect(reply.status()).toBe(202)
    await route.abort('connectionfailed')
  })
  await manager
    .locator('.mcp-tools')
    .getByRole('row')
    .filter({ hasText: toolName })
    .getByRole('button', { name: '调用', exact: true })
    .click()
  const calling = page.getByRole('dialog', { name: '调用 MCP 工具', exact: true })
  await calling.getByLabel('参数 JSON', { exact: true }).fill(
    JSON.stringify({
      caption: 'From plugin',
      recordKey: key,
      kind: 'string',
      payload: 'plugin-private-argument-marker',
    }),
  )
  await calling.getByRole('button', { name: '发送调用', exact: true }).click()
  await expect(calling.getByText('无法连接服务器，请检查地址和网络', { exact: true })).toBeVisible()
  await expect
    .poll(
      async () =>
        (await (await page.request.get(api + '/parameters?search=' + key, { headers })).json()).meta
          .total,
    )
    .toBe(1)
  expect(
    await page.evaluate(() =>
      Object.entries(localStorage)
        .filter(([key]) => key.startsWith('tri-admin.mcp.v1:'))
        .map(([, value]) => value)
        .join(''),
    ),
  ).not.toContain('plugin-private-argument-marker')
  await page.unrouteAll({ behavior: 'wait' })
  await page.reload()
  await login(page)
  await page.locator('nav').getByRole('button', { name: 'MCP连接', exact: true }).click()
  await page.getByRole('button', { name: '恢复查询', exact: true }).click()
  await expect(page.getByText('待确认 MCP 操作', { exact: true })).toHaveCount(0)
  manager = page.getByRole('dialog', { name: '连接管理', exact: true })
  const original = manager.locator('.mcp-history').getByRole('row').filter({ hasText: toolName })
  await expect(original.getByText('已返回', { exact: true })).toBeVisible()
  await original.getByRole('button', { name: '结果', exact: true }).click()
  const result = page.getByRole('dialog', { name: '工具结果', exact: true })
  await expect(result.locator('pre')).toContainText(key)
  await expect(result.getByText(/SHA-256 已核验/)).toBeVisible()
  await result.getByRole('button', { name: '关闭结果', exact: true }).click()
  await expect(result).not.toBeVisible()
  expect(writes).toBe(1)
  expect(
    (await (await page.request.get(api + '/parameters?search=' + key, { headers })).json()).meta
      .total,
  ).toBe(1)
  await manager.getByRole('button', { name: '停止', exact: true }).click()
  await expect(manager.getByText('已停止', { exact: true })).toBeVisible()
})

test('missing skill permission disables plugin execution and selection while retaining process cleanup', async ({
  page,
  browser,
}) => {
  test.setTimeout(60000)
  const token = await login(page),
    headers = { authorization: `Bearer ${token}` },
    extension = await install(
      page,
      token,
      'permission-' + Date.now().toString(36),
      [],
      [{ id: 'records', label: '权限来源', capabilityId: 'e2e-records', tools: ['inspect'] }],
    )
  expect(
    (
      await page.request.patch(`${api}/extensions/${extension.id}`, {
        headers,
        data: { version: 1, enabled: true },
      })
    ).status(),
  ).toBe(200)
  const response = await page.request.post(api + '/mcp/connections', {
    headers,
    data: {
      requestId: randomUUID(),
      name: '权限受控进程',
      enabled: true,
      transport: {
        kind: 'plugin',
        extensionId: extension.id,
        revisionId: extension.revisionId,
        contributionId: 'records',
      },
    },
  })
  expect(response.status()).toBe(201)
  const connection = (await response.json()).data
  const started = await page.request.post(`${api}/mcp/connections/${connection.id}/actions`, {
    headers,
    data: { requestId: randomUUID(), version: 1, kind: 'start' },
  })
  expect(started.status()).toBe(202)
  const action = (await started.json()).data
  await expect
    .poll(
      async () =>
        (await (await page.request.get(`${api}/mcp/actions/${action.id}`, { headers })).json()).data
          .status,
    )
    .toBe('succeeded')
  const role = await page.request.post(api + '/roles', {
    headers,
    data: {
      name: 'MCP operator without Skills',
      slug: 'source-no-skills-' + Date.now().toString(36),
      permissions: ['mcp.read', 'mcp.write', 'mcp.invoke'],
      dataScope: 'all',
    },
  })
  expect(role.status()).toBe(201)
  const account = 'no-skills-' + Date.now().toString(36)
  expect(
    (
      await page.request.post(api + '/users', {
        headers,
        data: {
          username: account,
          displayName: 'MCP 来源权限验证',
          password: await password(),
          roleIds: [(await role.json()).data.id],
        },
      })
    ).status(),
  ).toBe(201)
  const context = await browser.newContext()
  try {
    const operator = await context.newPage()
    await operator.addInitScript(
      (endpoint) => localStorage.setItem('tri-admin.server', endpoint),
      api,
    )
    await login(operator, account)
    await operator.locator('nav').getByRole('button', { name: 'MCP连接', exact: true }).click()
    await operator.getByRole('button', { name: '新建连接', exact: true }).click()
    const editor = operator.getByRole('dialog', { name: '新建 MCP 连接', exact: true })
    await editor.getByLabel('传输方式', { exact: true }).press('ArrowDown')
    await expect(operator.getByRole('option', { name: '插件 MCP', exact: true })).toHaveAttribute(
      'aria-disabled',
      'true',
    )
    await operator.keyboard.press('Escape')
    await editor.getByRole('button', { name: '关闭编辑', exact: true }).click()
    await operator
      .locator('.mcp-connections')
      .getByRole('row')
      .filter({ hasText: '权限受控进程' })
      .getByRole('button', { name: '管理', exact: true })
      .click()
    const manager = operator.getByRole('dialog', { name: '连接管理', exact: true })
    await expect(manager.getByRole('button', { name: '调用', exact: true })).toHaveCount(0)
    await expect(manager.getByRole('button', { name: '重启', exact: true })).toBeDisabled()
    await expect(manager.getByRole('button', { name: '刷新工具', exact: true })).toBeDisabled()
    await manager.getByRole('button', { name: '停止', exact: true }).click()
    await expect(manager.getByText('已停止', { exact: true })).toBeVisible()
  } finally {
    await context.close()
  }
})
