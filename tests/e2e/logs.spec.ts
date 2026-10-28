import { readFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { test, expect, type Page } from '@playwright/test'
let api = '',
  username = ''
async function password() {
  return readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8')
}
test.beforeEach(async ({ page }) => {
  api = process.env.TRI_E2E_API!
  username = 'logs-' + randomUUID().slice(0, 18)
  const response = await page.request.post(api + '/users', {
    headers: { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` },
    data: {
      username,
      displayName: '日志测试管理员',
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
  const pending = page.waitForResponse(
    (response) => response.url() === api + '/auth/login' && response.request().method() === 'POST',
  )
  await page.getByRole('button', { name: '登录', exact: true }).click()
  const response = await pending
  expect(response.status()).toBe(200)
  await expect(page.locator('.sidebar')).toBeVisible()
  return (await response.json()).data
}
async function logs(page: Page) {
  await page.locator('nav').getByRole('button', { name: '日志中心', exact: true }).click()
  await expect(page.getByRole('heading', { name: '日志中心', exact: true })).toBeVisible()
}
function localDate(ms: number) {
  const date = new Date(ms)
  return new Date(ms - date.getTimezoneOffset() * 60000).toISOString().slice(0, 16)
}

test('log center filters safe records and resumes a committed cleanup after a lost response and reload', async ({
  page,
}) => {
  test.setTimeout(60000)
  let auth = await login(page)
  let headers = { authorization: `Bearer ${auth.accessToken}` }
  const secret = 'UI-LOG-PRIVATE-' + randomUUID(),
    id = randomUUID()
  expect(
    (
      await page.request.post(api + '/file-categories?token=' + secret, {
        headers: { ...headers, 'x-request-id': id },
        data: { privateField: secret },
      })
    ).status(),
  ).toBe(422)
  await logs(page)
  await page.getByRole('tab', { name: '登录日志', exact: true }).click()
  await page.getByPlaceholder('账号精确匹配').fill(username)
  await page.getByRole('button', { name: '查询', exact: true }).click()
  await expect(page.locator('.logs-table')).toContainText(username)
  await page.getByRole('tab', { name: '错误日志', exact: true }).click()
  await page.getByPlaceholder('请求关联号 UUID').fill(id)
  await page.getByRole('button', { name: '查询', exact: true }).click()
  await expect(page.locator('.logs-table')).toContainText('VALIDATION_ERROR')
  await expect(page.locator('.logs-table')).toContainText('422')
  await expect(page.locator('body')).not.toContainText(secret)
  await page.getByRole('button', { name: '详情', exact: true }).first().click()
  const details = page.getByRole('dialog', { name: '日志详情', exact: true })
  await expect(details).toContainText('/api/v1/file-categories')
  await expect(details).toContainText(id)
  await details.getByRole('button', { name: '关闭详情', exact: true }).click()
  await page.getByRole('tab', { name: '操作日志', exact: true }).click()
  const attempts: string[] = []
  let first = true
  await page.route(api + '/logs/cleanup', async (route) => {
    attempts.push(route.request().postDataJSON().requestId)
    if (first) {
      first = false
      const response = await route.fetch()
      expect(response.status()).toBe(200)
      expect((await response.json()).data.deletedCount).toBe(2)
      await route.abort('failed')
    } else await route.continue()
  })
  await page.getByRole('button', { name: '清理历史', exact: true }).click()
  let dialog = page.getByRole('dialog', { name: '清理历史日志', exact: true })
  await dialog
    .getByLabel('清理时间之前', { exact: true })
    .fill(localDate(Date.now() - 2 * 86400000))
  await dialog.getByLabel('本批上限', { exact: true }).fill('2')
  await dialog.getByRole('button', { name: '清理本批', exact: true }).click()
  await expect(dialog).toContainText('结果尚未确认')
  await dialog.getByRole('button', { name: '关闭', exact: true }).click()
  await page.reload()
  auth = await login(page)
  headers = { authorization: `Bearer ${auth.accessToken}` }
  await logs(page)
  await expect(page.locator('.pending-cleanup')).toContainText('待确认')
  await page.getByRole('button', { name: '继续确认', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '清理历史日志', exact: true })
  await expect(dialog.getByLabel('清理时间之前', { exact: true })).toBeDisabled()
  await dialog.getByRole('button', { name: '重试原请求', exact: true }).click()
  await expect(dialog).toContainText('已清理 2 条')
  expect(attempts).toHaveLength(2)
  expect(attempts[0]).toBe(attempts[1])
  expect(
    (
      await (
        await page.request.get(api + '/logs?kind=operation&action=fixture.old', { headers })
      ).json()
    ).meta.total,
  ).toBe(1)
  expect(
    (
      await (
        await page.request.get(api + '/logs?kind=operation&action=logs.cleanup', { headers })
      ).json()
    ).meta.total,
  ).toBe(1)
})

test('audit readers can inspect global records without cleanup controls', async ({
  page,
  browser,
}) => {
  const owner = await login(page),
    headers = { authorization: `Bearer ${owner.accessToken}` },
    name = 'reader-' + randomUUID().slice(0, 18)
  const role = await page.request.post(api + '/roles', {
    headers,
    data: { name, slug: name, permissions: ['audit.read'], dataScope: 'self' },
  })
  expect(role.status()).toBe(201)
  const user = await page.request.post(api + '/users', {
    headers,
    data: {
      username: name,
      displayName: name,
      password: await password(),
      roleIds: [(await role.json()).data.id],
    },
  })
  expect(user.status()).toBe(201)
  const context = await browser.newContext()
  try {
    const reader = await context.newPage()
    await reader.addInitScript(
      (endpoint) => localStorage.setItem('tri-admin.server', endpoint),
      api,
    )
    await login(reader, name)
    await logs(reader)
    await expect(reader.getByRole('button', { name: '清理历史', exact: true })).toHaveCount(0)
    await expect(reader.locator('.logs-table')).toContainText('users.create')
  } finally {
    await context.close()
  }
})
