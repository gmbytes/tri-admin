import { readFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { randomUUID } from 'node:crypto'
import { test, expect, type Page, type Locator } from '@playwright/test'
let api = '',
  username = ''
async function password() {
  return readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8')
}
test.beforeEach(async ({ page }) => {
  api = process.env.TRI_E2E_API!
  username = 'settings-' + randomUUID().slice(0, 15)
  const response = await page.request.post(api + '/users', {
    headers: { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` },
    data: {
      username,
      displayName: '配置测试管理员',
      password: await password(),
      roleIds: [process.env.TRI_E2E_OWNER_ROLE],
    },
  })
  expect(response.status()).toBe(201)
  await page.addInitScript((endpoint) => localStorage.setItem('tri-admin.server', endpoint), api)
})
async function login(page: Page, account = username) {
  await page.goto('/#/login')
  await page.getByRole('textbox', { name: '账号', exact: true }).fill(account)
  await page.getByLabel('密码', { exact: true }).fill(await password())
  const ready = page.waitForResponse(
    (response) => response.url() === api + '/auth/login' && response.request().method() === 'POST',
  )
  await page.getByRole('button', { name: '登录', exact: true }).click()
  const response = await ready
  expect(response.status()).toBe(200)
  await expect(page.locator('.sidebar')).toBeVisible()
  return (await response.json()).data
}
async function settings(page: Page) {
  await page.locator('nav').getByRole('button', { name: '配置中心', exact: true }).click()
  await expect(page.getByRole('heading', { name: '配置中心', exact: true })).toBeVisible()
}
async function choose(page: Page, dialog: Locator, label: string, option: string) {
  const field = dialog.locator('.el-form-item').filter({ hasText: label })
  await field.locator('.el-select__wrapper').click()
  const id = await field.getByRole('combobox').getAttribute('aria-controls')
  await page.locator(`[id="${id}"]`).getByRole('option', { name: option, exact: true }).click()
}
test('configuration workspace applies service branding and manages write-only credentials and connection history', async ({
  page,
}) => {
  test.setTimeout(60000)
  let received = ''
  const endpoint = createServer((request, response) => {
    received = String(request.headers['x-fixture-key'] ?? '')
    response.writeHead(200)
    response.end()
  })
  await new Promise<void>((resolve) => endpoint.listen(0, '127.0.0.1', resolve))
  const address = endpoint.address()
  if (!address || typeof address === 'string') throw Error('missing server')
  try {
    const auth = await login(page),
      headers = { authorization: `Bearer ${auth.accessToken}` },
      suffix = Date.now().toString(36),
      name = '运营工作空间-' + suffix,
      code = 'ui-http-' + suffix,
      secret = 'PRIVATE-UI-' + suffix
    await settings(page)
    await page.getByLabel('应用名称', { exact: true }).fill(name)
    await page.getByRole('button', { name: '保存服务设置', exact: true }).click()
    await expect(page.locator('.sidebar .brand')).toContainText(name)
    await page.getByRole('tab', { name: '集成配置', exact: true }).click()
    await page.getByRole('button', { name: '新增配置', exact: true }).click()
    let dialog = page.getByRole('dialog', { name: '新增集成配置', exact: true })
    await choose(page, dialog, '配置类型', 'HTTP 服务')
    await dialog.getByLabel('配置名称', { exact: true }).fill('界面 HTTP ' + suffix)
    await dialog.getByLabel('配置标识', { exact: true }).fill(code)
    await dialog.getByLabel('基础地址', { exact: true }).fill(`http://127.0.0.1:${address.port}`)
    await dialog
      .locator('.el-form-item')
      .filter({ hasText: '启用配置' })
      .locator('.el-switch')
      .click()
    await dialog.getByText('替换凭据', { exact: true }).click()
    await dialog.getByLabel('授权头名称', { exact: true }).fill('X-Fixture-Key')
    await dialog.getByLabel('授权头值', { exact: true }).fill(secret)
    await dialog.getByRole('button', { name: '保存配置', exact: true }).click()
    await expect(dialog).not.toBeVisible()
    let row = page.getByRole('row').filter({ hasText: code })
    await expect(row).toContainText('已配置')
    await row.getByRole('button', { name: '检查连接', exact: true }).click()
    await expect(row).toContainText('检查通过')
    expect(received).toBe(secret)
    await row.getByRole('button', { name: '检查历史', exact: true }).click()
    const history = page.getByRole('dialog', { name: '连接检查历史', exact: true })
    await expect(history).toContainText('HTTP HEAD')
    await expect(history).not.toContainText(secret)
    await history.getByRole('button', { name: '关闭历史', exact: true }).click()
    await row.getByRole('button', { name: '编辑', exact: true }).click()
    dialog = page.getByRole('dialog', { name: '编辑集成配置', exact: true })
    await expect(dialog.getByLabel('授权头值', { exact: true })).toHaveCount(0)
    await dialog.getByLabel('配置名称', { exact: true }).fill('重命名 HTTP ' + suffix)
    await dialog.getByRole('button', { name: '保存配置', exact: true }).click()
    await expect(dialog).not.toBeVisible()
    await expect(row).toContainText('历史检查')
    await row.getByRole('button', { name: '编辑', exact: true }).click()
    dialog = page.getByRole('dialog', { name: '编辑集成配置', exact: true })
    await dialog.getByText('清除凭据', { exact: true }).click()
    await dialog.getByRole('button', { name: '保存配置', exact: true }).click()
    await expect(dialog).not.toBeVisible()
    await expect(row).toContainText('未配置')
    await page.getByRole('tab', { name: '启动状态', exact: true }).click()
    await expect(page.locator('.runtime-settings')).toContainText('活动加密密钥')
    await expect(page.locator('.runtime-settings')).not.toContainText(secret)
    await page.getByRole('tab', { name: '集成配置', exact: true }).click()
    row = page.getByRole('row').filter({ hasText: code })
    await row.getByRole('button', { name: '删除', exact: true }).click()
    await page.getByRole('button', { name: '删除配置', exact: true }).click()
    await expect(row).toHaveCount(0)
    const service = await page.request.get(api + '/settings/service', { headers })
    expect((await service.json()).data.displayName).toBe(name)
  } finally {
    endpoint.closeAllConnections()
    await new Promise<void>((resolve) => endpoint.close(() => resolve()))
  }
})
test('read-only settings users can inspect safe state without edit or probe controls', async ({
  page,
  browser,
}) => {
  const auth = await login(page),
    headers = { authorization: `Bearer ${auth.accessToken}` },
    name = 'cfg-reader-' + randomUUID().slice(0, 12)
  const role = await page.request.post(api + '/roles', {
    headers,
    data: { name, slug: name, permissions: ['settings.read'], dataScope: 'self' },
  })
  expect(role.status()).toBe(201)
  const created = await page.request.post(api + '/users', {
    headers,
    data: {
      username: name,
      displayName: name,
      password: await password(),
      roleIds: [(await role.json()).data.id],
    },
  })
  expect(created.status()).toBe(201)
  const context = await browser.newContext()
  try {
    const reader = await context.newPage()
    await reader.addInitScript(
      (endpoint) => localStorage.setItem('tri-admin.server', endpoint),
      api,
    )
    await login(reader, name)
    await settings(reader)
    await expect(reader.getByRole('button', { name: '保存服务设置', exact: true })).toHaveCount(0)
    await reader.getByRole('tab', { name: '集成配置', exact: true }).click()
    await expect(reader.getByRole('button', { name: '新增配置', exact: true })).toHaveCount(0)
    await expect(
      reader.locator('.table-surface').getByRole('button', { name: '检查连接', exact: true }),
    ).toHaveCount(0)
    await expect(reader.getByRole('button', { name: '编辑', exact: true })).toHaveCount(0)
  } finally {
    await context.close()
  }
})

test('typed integration editors persist each kind and service bindings preserve nullable policy', async ({
  page,
}) => {
  test.setTimeout(60000)
  const auth = await login(page),
    headers = { authorization: `Bearer ${auth.accessToken}` },
    suffix = Date.now().toString(36)
  await settings(page)
  await page.getByRole('tab', { name: '集成配置', exact: true }).click()
  const expected = [
    {
      label: 'MongoDB',
      code: `ui-db-${suffix}`,
      configuration: {
        kind: 'mongodb',
        hosts: [{ host: 'mongo.example.test', port: 27017 }],
        database: 'game',
        authDatabase: 'admin',
        replicaSet: 'rs-game',
        tls: true,
        directConnection: false,
      },
    },
    {
      label: '本地存储',
      code: `ui-local-${suffix}`,
      configuration: { kind: 'localStorage', subdirectory: `ui-${suffix}` },
    },
    {
      label: 'S3 对象存储',
      code: `ui-s3-${suffix}`,
      configuration: {
        kind: 's3',
        endpoint: 'https://objects.example.test',
        bucket: 'game-files',
        region: 'us-east-1',
        prefix: 'assets',
        pathStyle: false,
      },
    },
    {
      label: 'SMTP 邮件',
      code: `ui-mail-${suffix}`,
      configuration: {
        kind: 'smtp',
        host: 'mail.example.test',
        port: 465,
        security: 'tls',
        fromAddress: 'admin@example.test',
      },
    },
  ]
  const ids: Record<string, string> = {}
  for (const item of expected) {
    await page.getByRole('button', { name: '新增配置', exact: true }).click()
    const dialog = page.getByRole('dialog', { name: '新增集成配置', exact: true })
    if (item.configuration.kind === 'localStorage') {
      await dialog.getByText('替换凭据', { exact: true }).click()
      await dialog.getByLabel('授权头名称', { exact: true }).fill('X-Discarded-Key')
      await dialog.getByLabel('授权头值', { exact: true }).fill('discarded-draft-secret')
    }
    await choose(page, dialog, '配置类型', item.label)
    await dialog.getByLabel('配置名称', { exact: true }).fill(item.label + suffix)
    await dialog.getByLabel('配置标识', { exact: true }).fill(item.code)
    if (item.configuration.kind === 'mongodb') {
      await dialog.getByLabel('主机 1', { exact: true }).fill('mongo.example.test')
      await dialog.getByLabel('数据库', { exact: true }).fill('game')
      await dialog.getByLabel('副本集', { exact: true }).fill('rs-game')
    } else if (item.configuration.kind === 'localStorage') {
      await dialog.getByLabel('受控子目录', { exact: true }).fill(`ui-${suffix}`)
      await dialog
        .locator('.el-form-item')
        .filter({ hasText: '启用配置' })
        .locator('.el-switch')
        .click()
    } else if (item.configuration.kind === 's3') {
      await dialog.getByLabel('S3 端点', { exact: true }).fill('https://objects.example.test')
      await dialog.getByLabel('存储桶', { exact: true }).fill('game-files')
      await dialog.getByLabel('对象前缀', { exact: true }).fill('assets')
    } else {
      await dialog.getByLabel('SMTP 主机', { exact: true }).fill('mail.example.test')
      await dialog.getByLabel('发件地址', { exact: true }).fill('admin@example.test')
      await dialog
        .locator('.el-form-item')
        .filter({ hasText: '启用配置' })
        .locator('.el-switch')
        .click()
    }
    const created = page.waitForResponse(
      (response) =>
        response.url() === api + '/settings/profiles' && response.request().method() === 'POST',
      { timeout: 5000 },
    )
    await dialog.getByRole('button', { name: '保存配置', exact: true }).click()
    const response = await created
    expect(response.status()).toBe(201)
    const profile = (await response.json()).data
    expect(profile.configuration).toEqual(item.configuration)
    expect(profile.credentials.configured).toBe(false)
    ids[item.configuration.kind] = profile.id
    await expect(dialog).not.toBeVisible()
  }
  const local = expected[1]!,
    mail = expected[3]!
  const localRow = page.getByRole('row').filter({ hasText: local.code })
  await localRow.getByRole('button', { name: '编辑', exact: true }).click()
  const editor = page.getByRole('dialog', { name: '编辑集成配置', exact: true })
  await expect(editor.getByLabel('受控子目录', { exact: true })).toBeDisabled()
  await editor.getByRole('button', { name: '取消', exact: true }).click()
  const before = (await (await page.request.get(api + '/settings/service', { headers })).json())
    .data
  await page.getByRole('tab', { name: '服务设置', exact: true }).click()
  const form = page.locator('.settings-form')
  await choose(page, form, '默认存储', `本地存储${suffix} · ${local.code} · 本地存储`)
  await choose(page, form, '默认邮件配置', `SMTP 邮件${suffix} · ${mail.code} · SMTP 邮件`)
  await form
    .locator('.el-form-item')
    .filter({ hasText: '登录频率策略' })
    .locator('.el-switch')
    .click()
  await form.getByLabel('每 IP 每分钟登录上限', { exact: true }).fill('101')
  await page.getByRole('button', { name: '保存服务设置', exact: true }).click()
  await expect(
    page.getByText('当前生效上限：101 次 / IP / 分钟。保存后对下一次登录生效。', { exact: true }),
  ).toBeVisible()
  let service = (await (await page.request.get(api + '/settings/service', { headers })).json()).data
  expect(service).toMatchObject({
    storageProfileId: ids.localStorage,
    mailProfileId: ids.smtp,
    loginLimitPerIpOverride: 101,
  })
  await choose(page, form, '默认邮件配置', '不绑定')
  const resetPolicy = page.waitForResponse(
    (response) =>
      response.url() === api + '/settings/service' && response.request().method() === 'PATCH',
  )
  await form
    .locator('.el-form-item')
    .filter({ hasText: '登录频率策略' })
    .locator('.el-switch')
    .click()
  await page.getByRole('button', { name: '保存服务设置', exact: true }).click()
  expect((await resetPolicy).status()).toBe(200)
  await expect(form.getByLabel('每 IP 每分钟登录上限', { exact: true })).toHaveCount(0)
  await expect(form).toContainText('当前生效上限：100 次 / IP / 分钟')
  service = (await (await page.request.get(api + '/settings/service', { headers })).json()).data
  expect(service.mailProfileId).toBeNull()
  expect(service.loginLimitPerIpOverride).toBeNull()
  expect(service.effectiveLoginLimitPerIp).toBe(100)
  expect(
    (
      await page.request.patch(api + '/settings/service', {
        headers,
        data: { version: service.version, storageProfileId: before.storageProfileId },
      })
    ).ok(),
  ).toBeTruthy()
})
