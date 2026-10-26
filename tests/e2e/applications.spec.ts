import { test, expect, type Page } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'

const scopes = ['open.profile.read', 'open.profile.write', 'open.users.read', 'open.users.write']
const scopeLabels = ['读取本人资料', '修改本人资料', '读取应用用户', '管理应用用户']
async function login(page: Page, permissions?: string[]) {
  const api = process.env.TRI_E2E_API!,
    password = await readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8')
  const suffix = randomUUID().replaceAll('-', ''),
    headers = { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` }
  let role = process.env.TRI_E2E_OWNER_ROLE
  if (permissions) {
    const response = await page.request.post(api + '/roles', {
      headers,
      data: { name: '访问验证', slug: 'access-' + suffix, permissions, dataScope: 'all' },
    })
    expect(response.status()).toBe(201)
    role = (await response.json()).data.id
  }
  const username = 'app-' + suffix
  expect(
    (
      await page.request.post(api + '/users', {
        headers,
        data: { username, displayName: '应用管理员', password, roleIds: [role] },
      })
    ).status(),
  ).toBe(201)
  await page.addInitScript((endpoint) => localStorage.setItem('tri-admin.server', endpoint), api)
  await page.goto('/#/login')
  await page.getByRole('textbox', { name: '账号', exact: true }).fill(username)
  await page.getByLabel('密码', { exact: true }).fill(password)
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await expect(page.locator('.sidebar')).toBeVisible()
  return { api, headers, suffix, password }
}
async function applications(page: Page) {
  await page.locator('nav').getByRole('button', { name: '开放应用', exact: true }).click()
  await expect(page.getByRole('heading', { name: '开放应用', exact: true })).toBeVisible()
  await page.getByRole('button', { name: '刷新应用', exact: true }).click()
}
async function create(page: Page, code: string) {
  await page.getByRole('button', { name: '创建应用', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: '创建应用', exact: true })
  await dialog.getByRole('textbox', { name: '应用名称', exact: true }).fill('游戏应用-' + code)
  await dialog.getByRole('textbox', { name: '应用标识', exact: true }).fill(code)
  for (const label of scopeLabels) {
    const checkbox = dialog.getByRole('checkbox', { name: label, exact: true })
    if (!(await checkbox.isChecked()))
      await dialog.locator('.el-checkbox').filter({ hasText: label }).click()
    await expect(checkbox).toBeChecked()
  }
  await dialog.getByRole('button', { name: '保存应用', exact: true }).click()
  const once = page.getByRole('dialog', { name: '一次性凭据', exact: true })
  await expect(once).toBeVisible()
  const secret = await once.getByRole('textbox', { name: '一次性凭据', exact: true }).inputValue(),
    id = secret.split('.')[1]!
  expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toContain(secret)
  await once.getByRole('button', { name: '我已保存，关闭', exact: true }).click()
  return {
    secret,
    id,
    row: page.locator('.applications-table').getByRole('row').filter({ hasText: code }),
  }
}

test('application UI manages isolated users and real open credentials with profile, service scopes, logs and rotation', async ({
  page,
}) => {
  test.setTimeout(60000)
  const { api, suffix, password } = await login(page)
  await applications(page)
  const code = 'game-' + suffix,
    app = await create(page, code),
    open = api.replace('/api/v1', '/open/v1')
  await app.row.getByRole('button', { name: '用户', exact: true }).click()
  const panel = page.getByRole('dialog', { name: '应用用户', exact: true })
  await panel.getByRole('button', { name: '新增应用用户', exact: true }).click()
  const userDialog = page.getByRole('dialog', { name: '新增应用用户', exact: true })
  await userDialog.getByRole('textbox', { name: '用户账号', exact: true }).fill('player.one')
  await userDialog.getByRole('textbox', { name: '用户显示名', exact: true }).fill('玩家一')
  await userDialog.getByLabel('初始密码', { exact: true }).fill(password)
  await userDialog.locator('.el-checkbox').filter({ hasText: '修改本人资料' }).click()
  await userDialog.getByRole('button', { name: '保存应用用户', exact: true }).click()
  await expect(userDialog).not.toBeVisible()
  await expect(panel.getByRole('row').filter({ hasText: 'player.one' })).toContainText('玩家一')
  const issued = await page.request.post(open + '/auth/application', {
    data: { applicationId: app.id, secret: app.secret, scopes },
  })
  expect(issued.status()).toBe(200)
  const service = (await issued.json()).data.accessToken,
    serviceHeaders = { authorization: `Bearer ${service}` }
  const users = await page.request.get(open + '/users', { headers: serviceHeaders })
  expect(users.status()).toBe(200)
  const user = (await users.json()).data[0]
  expect(user.username).toBe('player.one')
  const bot = await page.request.post(open + '/users', {
    headers: serviceHeaders,
    data: {
      username: 'service.bot',
      displayName: '服务创建',
      password,
      scopes: ['open.profile.read'],
    },
  })
  expect(bot.status()).toBe(201)
  const botId = (await bot.json()).data.id
  expect(
    (
      await page.request.delete(`${open}/users/${botId}?version=1`, { headers: serviceHeaders })
    ).status(),
  ).toBe(204)
  const signed = await page.request.post(open + '/auth/login', {
    data: {
      applicationId: app.id,
      username: 'player.one',
      password,
      scopes: ['open.profile.read', 'open.profile.write'],
    },
  })
  expect(signed.status()).toBe(200)
  const player = (await signed.json()).data.accessToken,
    playerHeaders = { authorization: `Bearer ${player}` }
  expect((await page.request.get(open + '/me', { headers: playerHeaders })).status()).toBe(200)
  expect(
    (
      await page.request.patch(open + '/me', {
        headers: playerHeaders,
        data: { version: 1, displayName: '玩家更新' },
      })
    ).status(),
  ).toBe(200)
  expect((await page.request.get(open + '/users', { headers: playerHeaders })).status()).toBe(403)
  await panel.getByRole('button', { name: '刷新用户', exact: true }).click()
  await expect(panel.getByRole('row').filter({ hasText: 'player.one' })).toContainText('玩家更新')
  await panel.getByRole('button', { name: '关闭此对话框', exact: true }).click()
  await app.row.getByRole('button', { name: '调用', exact: true }).click()
  const logs = page.getByRole('dialog', { name: '应用调用记录', exact: true })
  await expect(logs.locator('.access-call-history')).toContainText('open.profile')
  await logs.getByRole('spinbutton', { name: 'HTTP 状态筛选', exact: true }).fill('403')
  await logs.getByRole('button', { name: '查询记录', exact: true }).click()
  await expect(logs.locator('.el-table__body')).toContainText('HTTP 403')
  await expect(logs.locator('.el-table__body')).not.toContainText('HTTP 200')
  await logs.getByRole('button', { name: '关闭此对话框', exact: true }).click()
  await app.row.getByRole('button', { name: '轮换密钥', exact: true }).click()
  await page
    .getByRole('dialog', { name: '轮换应用密钥', exact: true })
    .getByRole('button', { name: '轮换', exact: true })
    .click()
  const once = page.getByRole('dialog', { name: '一次性凭据', exact: true })
  await expect(once).toBeVisible()
  const nextSecret = await once
    .getByRole('textbox', { name: '一次性凭据', exact: true })
    .inputValue()
  expect(nextSecret).not.toBe(app.secret)
  expect((await page.request.get(open + '/users', { headers: serviceHeaders })).status()).toBe(401)
  expect(
    (
      await page.request.post(open + '/auth/application', {
        data: { applicationId: app.id, secret: app.secret, scopes },
      })
    ).status(),
  ).toBe(401)
  expect(
    (
      await page.request.post(open + '/auth/application', {
        data: { applicationId: app.id, secret: nextSecret, scopes },
      })
    ).status(),
  ).toBe(200)
  await once.getByRole('button', { name: '我已保存，关闭', exact: true }).click()
  await app.row.getByRole('button', { name: '用户', exact: true }).click()
  await panel
    .getByRole('row')
    .filter({ hasText: 'player.one' })
    .getByRole('button', { name: '重置密码', exact: true })
    .click()
  const reset = page.getByRole('dialog', { name: '重置应用用户密码', exact: true }),
    nextPassword = password + '-reset'
  await reset.getByLabel('新密码', { exact: true }).fill(nextPassword)
  await reset.getByRole('button', { name: '重置密码', exact: true }).click()
  await expect(reset).not.toBeVisible()
  expect((await page.request.get(open + '/me', { headers: playerHeaders })).status()).toBe(401)
  expect(
    (
      await page.request.post(open + '/auth/login', {
        data: { applicationId: app.id, username: 'player.one', password: nextPassword },
      })
    ).status(),
  ).toBe(200)
  await panel.getByRole('button', { name: '关闭此对话框', exact: true }).click()
  const second = await create(page, 'second-' + suffix)
  await second.row.getByRole('button', { name: '用户', exact: true }).click()
  await expect(panel.getByText('共 0 个应用用户', { exact: true })).toBeVisible()
  await expect(panel.getByRole('row').filter({ hasText: 'player.one' })).toHaveCount(0)
  await panel.getByRole('button', { name: '新增应用用户', exact: true }).click()
  await userDialog.getByRole('textbox', { name: '用户账号', exact: true }).fill('player.one')
  await userDialog.getByRole('textbox', { name: '用户显示名', exact: true }).fill('第二应用玩家')
  await userDialog.getByLabel('初始密码', { exact: true }).fill(password)
  await userDialog.getByRole('button', { name: '保存应用用户', exact: true }).click()
  await expect(userDialog).not.toBeVisible()
  await expect(panel.getByRole('row').filter({ hasText: 'player.one' })).toContainText(
    '第二应用玩家',
  )
  await expect(panel).not.toContainText('玩家更新')
  await page.screenshot({
    path: '.superpowers/sdd/2026-10-24-tri-admin-greenfield/application-users-ui.png',
    fullPage: true,
  })
})

test('configuration only editor sends changed fields and explicitly reloads a stale application version', async ({
  page,
}) => {
  const { api, headers, suffix } = await login(page, ['applications.read', 'applications.write'])
  const response = await page.request.post(api + '/applications', {
    headers,
    data: {
      requestId: randomUUID(),
      name: '配置原名称',
      code: 'config-' + suffix,
      allowedScopes: ['open.profile.read'],
    },
  })
  expect(response.status()).toBe(201)
  const app = (await response.json()).data.application
  await applications(page)
  await expect(page.getByRole('button', { name: '创建应用', exact: true })).toHaveCount(0)
  const row = page
    .locator('.applications-table')
    .getByRole('row')
    .filter({ hasText: 'config-' + suffix })
  await row.getByRole('button', { name: '编辑', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: '编辑应用', exact: true })
  await expect(dialog.getByRole('checkbox', { name: '读取本人资料', exact: true })).toBeDisabled()
  await expect(row.getByRole('button', { name: '轮换密钥', exact: true })).toHaveCount(0)
  expect(
    (
      await page.request.patch(`${api}/applications/${app.id}`, {
        headers,
        data: { version: 1, name: '另一管理员更新' },
      })
    ).status(),
  ).toBe(200)
  await dialog.getByRole('textbox', { name: '应用名称', exact: true }).fill('我的修改')
  const stale = page.waitForResponse(
    (response) =>
      response.url() === `${api}/applications/${app.id}` && response.request().method() === 'PATCH',
  )
  await dialog.getByRole('button', { name: '保存应用', exact: true }).click()
  expect((await stale).status()).toBe(409)
  await expect(dialog).toBeVisible()
  await expect(dialog.getByRole('alert')).toBeVisible()
  await dialog.getByRole('button', { name: '重新读取', exact: true }).click()
  await expect(dialog.getByRole('textbox', { name: '应用名称', exact: true })).toHaveValue(
    '另一管理员更新',
  )
  await dialog.getByRole('textbox', { name: '应用名称', exact: true }).fill('已核对的新名称')
  const saved = page.waitForRequest(
    (request) => request.url() === `${api}/applications/${app.id}` && request.method() === 'PATCH',
  )
  await dialog.getByRole('button', { name: '保存应用', exact: true }).click()
  expect((await saved).postDataJSON()).toEqual({ version: 2, name: '已核对的新名称' })
  await expect(dialog).not.toBeVisible()
  await expect(row).toContainText('已核对的新名称')
})

test('lost rotation acknowledgement recovers a fixed receipt and a later explicit rotation has a new identity', async ({
  page,
}) => {
  const { suffix } = await login(page)
  await applications(page)
  const app = await create(page, 'rotate-' + suffix)
  let writes = 0,
    completed = false,
    firstRequest = ''
  await page.route(`**/api/v1/applications/${app.id}/secret`, async (route) => {
    writes++
    firstRequest = route.request().postDataJSON().requestId
    await route.fetch()
    await route.abort()
    completed = true
  })
  await app.row.getByRole('button', { name: '轮换密钥', exact: true }).click()
  await page
    .getByRole('dialog', { name: '轮换应用密钥', exact: true })
    .getByRole('button', { name: '轮换', exact: true })
    .click()
  await expect.poll(() => completed).toBe(true)
  await expect(page.getByText('待确认的凭据操作', { exact: true })).toBeVisible()
  expect(writes).toBe(1)
  await expect(page.getByRole('dialog', { name: '一次性凭据', exact: true })).toHaveCount(0)
  const marker = await page.evaluate(() =>
    Object.entries(localStorage)
      .filter(([key]) => key.startsWith('tri-admin.credentials.'))
      .map(([, value]) => value)
      .join(''),
  )
  expect(marker).toContain(firstRequest)
  expect(marker).toContain('"version":1')
  expect(marker).not.toContain(app.secret)
  await page.unrouteAll({ behavior: 'wait' })
  await page.getByRole('button', { name: '恢复查询', exact: true }).click()
  await expect(page.getByText('待确认的凭据操作', { exact: true })).not.toBeVisible()
  await expect(app.row).toContainText('凭据 2')
  await app.row.getByRole('button', { name: '轮换密钥', exact: true }).click()
  const request = page.waitForRequest(
    (request) =>
      request.url().endsWith(`/applications/${app.id}/secret`) && request.method() === 'POST',
  )
  await page
    .getByRole('dialog', { name: '轮换应用密钥', exact: true })
    .getByRole('button', { name: '轮换', exact: true })
    .click()
  expect((await request).postDataJSON().requestId).not.toBe(firstRequest)
  await expect(page.getByRole('dialog', { name: '一次性凭据', exact: true })).toBeVisible()
  await page.goto('/#/profile')
  await expect(page.getByRole('heading', { name: '个人中心', exact: true })).toBeVisible()
  await expect(page.getByRole('dialog', { name: '一次性凭据', exact: true })).toHaveCount(0)
  await applications(page)
  await expect(page.getByRole('dialog', { name: '一次性凭据', exact: true })).toHaveCount(0)
  await expect(app.row).toContainText('凭据 3')
})

test('read only application and token pages expose no credential or user mutation controls', async ({
  page,
}) => {
  const { api, headers, suffix } = await login(page, [
    'applications.read',
    'applicationUsers.read',
    'applicationLogs.read',
    'apiTokens.read',
  ])
  const response = await page.request.post(api + '/applications', {
    headers,
    data: {
      requestId: randomUUID(),
      name: '只读应用',
      code: 'readonly-' + suffix,
      allowedScopes: ['open.profile.read'],
    },
  })
  expect(response.status()).toBe(201)
  await applications(page)
  await expect(page.getByRole('button', { name: '创建应用', exact: true })).toHaveCount(0)
  const row = page
    .locator('.applications-table')
    .getByRole('row')
    .filter({ hasText: 'readonly-' + suffix })
  await expect(row).toBeVisible()
  for (const name of ['编辑', '轮换密钥', '删除'])
    await expect(row.getByRole('button', { name, exact: true })).toHaveCount(0)
  await row.getByRole('button', { name: '用户', exact: true }).click()
  const users = page.getByRole('dialog', { name: '应用用户', exact: true })
  await expect(users.getByRole('button', { name: '新增应用用户', exact: true })).toHaveCount(0)
  await users.getByRole('button', { name: '关闭此对话框', exact: true }).click()
  await page.locator('nav').getByRole('button', { name: 'API Token', exact: true }).click()
  await expect(page.getByRole('button', { name: '签发 Token', exact: true })).toHaveCount(0)
})

test('application, user and call pages retain server pagination and controlled filters', async ({
  page,
}) => {
  test.setTimeout(90000)
  const { api, headers, suffix, password } = await login(page, [
    'applications.read',
    'applicationUsers.read',
    'applicationLogs.read',
  ])
  const prefix = 'paging-' + suffix
  let app: { id: string; code: string } | undefined,
    secret = ''
  for (let index = 0; index < 21; index++) {
    const response = await page.request.post(api + '/applications', {
      headers,
      data: {
        requestId: randomUUID(),
        name: '分页应用 ' + index,
        code: prefix + '-' + String(index).padStart(2, '0'),
        allowedScopes: ['open.profile.read', 'open.users.read'],
      },
    })
    expect(response.status()).toBe(201)
    const issued = (await response.json()).data
    app = issued.application
    secret = issued.secret
  }
  for (let index = 0; index < 21; index++)
    expect(
      (
        await page.request.post(`${api}/applications/${app!.id}/users`, {
          headers,
          data: {
            username: 'player.' + String(index).padStart(2, '0'),
            displayName: '分页玩家 ' + index,
            password,
          },
        })
      ).status(),
    ).toBe(201)
  const open = api.replace('/api/v1', '/open/v1'),
    issued = await page.request.post(open + '/auth/application', {
      data: { applicationId: app!.id, secret, scopes: ['open.users.read'] },
    })
  expect(issued.status()).toBe(200)
  const access = (await issued.json()).data.accessToken
  for (let index = 0; index < 21; index++)
    expect(
      (
        await page.request.get(open + '/users', { headers: { authorization: `Bearer ${access}` } })
      ).status(),
    ).toBe(200)
  await applications(page)
  await page.getByPlaceholder('按应用标识前缀搜索', { exact: true }).fill(prefix)
  await page.getByRole('button', { name: '查询', exact: true }).click()
  await expect(page.getByText('共 21 个应用', { exact: true })).toBeVisible()
  await expect(page.locator('.applications-table .el-table__body tbody tr')).toHaveCount(20)
  await page.getByRole('button', { name: '下一页', exact: true }).click()
  await expect(page.locator('.applications-table .el-table__body tbody tr')).toHaveCount(1)
  await page.getByRole('button', { name: '上一页', exact: true }).click()
  const row = page.locator('.applications-table').getByRole('row').filter({ hasText: app!.code })
  await row.getByRole('button', { name: '用户', exact: true }).click()
  const users = page.getByRole('dialog', { name: '应用用户', exact: true })
  await expect(users.getByText('共 21 个应用用户', { exact: true })).toBeVisible()
  await users.getByRole('button', { name: '下一页', exact: true }).click()
  await expect(users.locator('.el-table__body tbody tr')).toHaveCount(1)
  await users.getByPlaceholder('按用户账号前缀搜索', { exact: true }).fill('player.0')
  await users.getByRole('button', { name: '查询用户', exact: true }).click()
  await expect(users.getByText('共 10 个应用用户', { exact: true })).toBeVisible()
  await expect(users.locator('.el-table__body tbody tr')).toHaveCount(10)
  await users.getByRole('button', { name: '关闭此对话框', exact: true }).click()
  await row.getByRole('button', { name: '调用', exact: true }).click()
  const logs = page.getByRole('dialog', { name: '应用调用记录', exact: true })
  await expect(logs.getByText('共 21 条', { exact: true })).toBeVisible()
  await logs.getByRole('button', { name: '下一页', exact: true }).click()
  await expect(logs.locator('.el-table__body tbody tr')).toHaveCount(1)
  await logs
    .getByRole('textbox', { name: '用户 ID 筛选', exact: true })
    .fill('000000000000000000000000')
  await logs.getByRole('button', { name: '查询记录', exact: true }).click()
  await expect(logs.getByText('共 0 条', { exact: true })).toBeVisible()
  await expect(logs.locator('.el-table__body tbody tr')).toHaveCount(0)
})

test('an application user write keeps its form bound until the pending result is confirmed', async ({
  page,
}) => {
  const { suffix, password } = await login(page)
  await applications(page)
  const app = await create(page, 'pending-' + suffix)
  await app.row.getByRole('button', { name: '用户', exact: true }).click()
  const panel = page.getByRole('dialog', { name: '应用用户', exact: true })
  await panel.getByRole('button', { name: '新增应用用户', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: '新增应用用户', exact: true })
  await dialog.getByRole('textbox', { name: '用户账号', exact: true }).fill('pending.player')
  await dialog.getByRole('textbox', { name: '用户显示名', exact: true }).fill('待确认玩家')
  await dialog.getByLabel('初始密码', { exact: true }).fill(password)
  let committed = false,
    release!: () => void
  const held = new Promise<void>((resolve) => {
    release = resolve
  })
  await page.route(`**/api/v1/applications/${app.id}/users`, async (route) => {
    if (route.request().method() !== 'POST') return route.continue()
    const response = await route.fetch()
    committed = true
    await held
    await route.fulfill({ response })
  })
  await dialog.getByRole('button', { name: '保存应用用户', exact: true }).click()
  try {
    await expect.poll(() => committed).toBe(true)
    await expect(dialog.getByRole('button', { name: '取消', exact: true })).toBeDisabled()
    await page.keyboard.press('Escape')
    await expect(dialog).toBeVisible()
  } finally {
    release()
    await expect(dialog).not.toBeVisible()
  }
  await expect(panel.getByRole('row').filter({ hasText: 'pending.player' })).toContainText(
    '待确认玩家',
  )
})
