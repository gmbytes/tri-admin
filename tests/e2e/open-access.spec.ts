import { test, expect, type Page } from '@playwright/test'
import { readFile } from 'node:fs/promises'
async function login(page: Page, existing?: { username: string; password: string }) {
  const api = process.env.TRI_E2E_API!,
    password = existing?.password ?? (await readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8')),
    username = existing?.username ?? 'access-' + Date.now().toString(36)
  if (!existing)
    expect(
      (
        await page.request.post(api + '/users', {
          headers: { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` },
          data: {
            username,
            displayName: '访问管理验证',
            password,
            roleIds: [process.env.TRI_E2E_OWNER_ROLE],
          },
        })
      ).status(),
    ).toBe(201)
  await page.addInitScript((endpoint) => localStorage.setItem('tri-admin.server', endpoint), api)
  await page.goto('/#/login')
  await page.getByRole('textbox', { name: '账号', exact: true }).fill(username)
  await page.getByLabel('密码', { exact: true }).fill(password)
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await expect(page.locator('.sidebar')).toBeVisible()
  return { api, username, password }
}
async function tokens(page: Page) {
  const errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  await page.locator('nav').getByRole('button', { name: 'API Token', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'API Token', exact: true }))
    .toBeVisible()
    .catch((error) => {
      throw new Error(errors.join('\n') || error.message)
    })
}
async function choose(page: Page, name: string, label: string) {
  const field = page.getByRole('combobox', { name, exact: true })
  await field.locator('xpath=ancestor::div[contains(@class,"el-select__wrapper")]').click()
  const controls = await field.getAttribute('aria-controls')
  await page.locator(`[id="${controls}"]`).getByRole('option', { name: label, exact: true }).click()
  await page.keyboard.press('Escape')
}
test('token workspace issues one secret and a real client obeys its scopes before revocation', async ({
  page,
}) => {
  const { api } = await login(page)
  await tokens(page)
  await page.getByRole('button', { name: '签发 Token', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: '签发 Token', exact: true })
  await dialog.getByRole('textbox', { name: 'Token 名称', exact: true }).fill('受控自动化')
  await choose(page, 'Token Scope', '查看参数 (parameters.read)')
  await dialog.getByLabel('来源 CIDR', { exact: true }).fill('127.0.0.1/32')
  await dialog.getByRole('spinbutton', { name: '每分钟限额', exact: true }).fill('10')
  await dialog.getByRole('spinbutton', { name: '每日配额', exact: true }).fill('20')
  await dialog.getByRole('button', { name: '签发', exact: true }).click()
  const once = page.getByRole('dialog', { name: '一次性凭据', exact: true })
  await expect(once).toBeVisible()
  const secret = await once.getByRole('textbox', { name: '一次性凭据', exact: true }).inputValue()
  expect(secret.startsWith('tri_at1.')).toBe(true)
  expect(
    (
      await page.request.get(api + '/parameters', {
        headers: { authorization: `Bearer ${secret}` },
      })
    ).status(),
  ).toBe(200)
  expect(
    (
      await page.request.get(api + '/users', { headers: { authorization: `Bearer ${secret}` } })
    ).status(),
  ).toBe(403)
  expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toContain(secret)
  await once.getByRole('button', { name: '我已保存，关闭', exact: true }).click()
  await expect(once).not.toBeVisible()
  const row = page.getByRole('row').filter({ hasText: '受控自动化' })
  await expect(row).toContainText('有效')
  await expect(row).toContainText('10 / 分钟')
  await expect(row).toContainText('20 / UTC 日')
  await expect(row).toContainText('127.0.0.1/32')
  await row.getByRole('button', { name: '撤销', exact: true }).click()
  await page
    .getByRole('dialog', { name: '撤销 Token', exact: true })
    .getByRole('button', { name: '撤销', exact: true })
    .click()
  await expect(row).toContainText('已撤销')
  expect(
    (
      await page.request.get(api + '/parameters', {
        headers: { authorization: `Bearer ${secret}` },
      })
    ).status(),
  ).toBe(401)
  await expect(page.getByRole('dialog', { name: '撤销 Token', exact: true })).not.toBeVisible()
  await page.screenshot({
    path: '.superpowers/sdd/2026-10-24-tri-admin-greenfield/token-ui.png',
    fullPage: true,
  })
})
test('lost issuance acknowledgement reloads into metadata recovery without a second POST or secret persistence', async ({
  page,
}) => {
  const account = await login(page)
  await tokens(page)
  let writes = 0,
    request = '',
    completed = false
  await page.route('**/api/v1/api-tokens', async (route) => {
    if (route.request().method() !== 'POST') return route.continue()
    writes++
    request = route.request().postDataJSON().requestId
    await route.fetch()
    await route.abort()
    completed = true
  })
  await page.getByRole('button', { name: '签发 Token', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: '签发 Token', exact: true })
  await dialog.getByRole('textbox', { name: 'Token 名称', exact: true }).fill('丢失响应')
  await choose(page, 'Token Scope', '查看参数 (parameters.read)')
  await dialog.getByRole('button', { name: '签发', exact: true }).click()
  await expect(page.getByText('待确认的凭据操作', { exact: true })).toBeVisible()
  await expect.poll(() => completed).toBe(true)
  expect(writes).toBe(1)
  const stored = await page.evaluate(() =>
    Object.entries(localStorage)
      .filter(([key]) => key.startsWith('tri-admin.credentials.'))
      .map(([, value]) => value)
      .join(''),
  )
  expect(stored).toContain(request)
  expect(stored).not.toContain('parameters.read')
  expect(stored).not.toContain('丢失响应')
  await page.unroute('**/api/v1/api-tokens')
  await page.reload()
  await login(page, account)
  await tokens(page)
  await page.getByRole('button', { name: '恢复查询', exact: true }).click()
  await expect(page.getByText('待确认的凭据操作', { exact: true })).not.toBeVisible()
  await expect(page.getByRole('row').filter({ hasText: '丢失响应' })).toBeVisible()
  await expect(page.getByRole('dialog', { name: '一次性凭据', exact: true })).toHaveCount(0)
  expect(writes).toBe(1)
})

for (const transition of ['logout', 'server'] as const) {
  test(`one time secret is cleared when the live session performs ${transition}`, async ({
    page,
  }) => {
    const account = await login(page)
    await tokens(page)
    await page.getByRole('button', { name: '签发 Token', exact: true }).click()
    const dialog = page.getByRole('dialog', { name: '签发 Token', exact: true })
    await dialog.getByRole('textbox', { name: 'Token 名称', exact: true }).fill('会话边界验证')
    await choose(page, 'Token Scope', '查看参数 (parameters.read)')
    await dialog.getByRole('button', { name: '签发', exact: true }).click()
    const once = page.getByRole('dialog', { name: '一次性凭据', exact: true })
    await expect(once).toBeVisible()
    const secret = await once.getByRole('textbox', { name: '一次性凭据', exact: true }).inputValue()
    await page.evaluate(
      async ({ transition, api }) => {
        const path = '/src/stores/session.ts'
        const { useSession } = (await import(
          /* @vite-ignore */ path
        )) as typeof import('../../apps/desktop/src/stores/session')
        const session = useSession()
        if (transition === 'logout') await session.logout()
        else await session.changeServer(api.replace('127.0.0.1', 'localhost'))
      },
      { transition, api: account.api },
    )
    await expect(once).not.toBeVisible()
    expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toContain(secret)
    await login(page)
    await tokens(page)
    await expect(page.getByRole('dialog', { name: '一次性凭据', exact: true })).toHaveCount(0)
  })
}
