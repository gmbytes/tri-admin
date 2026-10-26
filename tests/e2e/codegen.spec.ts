import { test, expect, type Page } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { randomUUID, createHash } from 'node:crypto'
async function login(
  page: Page,
  permissions?: string[],
  existing?: { username: string; password: string },
) {
  const api = process.env.TRI_E2E_API!,
    password = existing?.password ?? (await readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8')),
    username = existing?.username ?? 'codegen-' + randomUUID().replaceAll('-', ''),
    headers = { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` }
  let role = process.env.TRI_E2E_OWNER_ROLE
  if (!existing) {
    if (permissions) {
      const response = await page.request.post(api + '/roles', {
        headers,
        data: {
          name: '生成器权限',
          slug: 'codegen-' + randomUUID().replaceAll('-', ''),
          permissions,
          dataScope: 'all',
        },
      })
      expect(response.status()).toBe(201)
      role = (await response.json()).data.id
    }
    expect(
      (
        await page.request.post(api + '/users', {
          headers,
          data: { username, displayName: '生成器验证', password, roleIds: [role] },
        })
      ).status(),
    ).toBe(201)
  }
  await page.addInitScript((endpoint) => localStorage.setItem('tri-admin.server', endpoint), api)
  await page.goto('/#/login')
  await page.getByRole('textbox', { name: '账号', exact: true }).fill(username)
  await page.getByLabel('密码', { exact: true }).fill(password)
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await expect(page.locator('.sidebar')).toBeVisible()
  return { api, headers, username, password }
}
async function workspace(page: Page) {
  await page.locator('nav').getByRole('button', { name: '代码生成', exact: true }).click()
  await expect(page.getByRole('heading', { name: '代码生成', exact: true })).toBeVisible()
  await page.getByRole('button', { name: '刷新定义', exact: true }).click()
}
async function create(page: Page, resource: string) {
  await page.getByRole('button', { name: '新建定义', exact: true }).click()
  const form = page.getByRole('dialog', { name: '新建生成定义', exact: true })
  await form.getByRole('textbox', { name: '资源标识', exact: true }).fill(resource)
  await form.getByRole('textbox', { name: '显示名称', exact: true }).fill('生成物品')
  await form.getByRole('textbox', { name: '字段标识 1', exact: true }).fill('sku')
  await form.getByRole('textbox', { name: '字段名称 1', exact: true }).fill('编号')
  await form.getByRole('button', { name: '保存定义', exact: true }).click()
  await expect(form).not.toBeVisible()
  return page.locator('.codegen-definitions').getByRole('row').filter({ hasText: resource })
}
test('code generation workspace edits definitions and downloads the exact immutable project', async ({
  page,
}) => {
  const { api } = await login(page)
  await workspace(page)
  const resource = 'inventory_' + randomUUID().replaceAll('-', '').slice(0, 12),
    row = await create(page, resource)
  const generated = page.waitForResponse(
    (response) =>
      response.url() === api + '/codegen/generations' && response.request().method() === 'POST',
  )
  await row.getByRole('button', { name: '生成工程', exact: true }).click()
  const response = await generated
  expect(response.status()).toBe(201)
  const result = (await response.json()).data
  const preview = page.getByRole('dialog', { name: '生成工程预览', exact: true })
  await expect(preview).toBeVisible()
  await expect(preview.getByRole('textbox', { name: '源码预览', exact: true })).toHaveValue(
    /独立工程/,
  )
  const downloading = page.waitForEvent('download')
  await preview.getByRole('button', { name: '下载 ZIP', exact: true }).click()
  const download = await downloading,
    path = await download.path()
  expect(path).toBeTruthy()
  const bytes = await readFile(path!)
  expect(createHash('sha256').update(bytes).digest('hex')).toBe(result.sha256)
  expect(bytes.length).toBe(result.size)
  await preview.getByRole('button', { name: '关闭此对话框', exact: true }).click()
  await row.getByRole('button', { name: '编辑', exact: true }).click()
  const edit = page.getByRole('dialog', { name: '编辑生成定义', exact: true })
  await edit.getByRole('textbox', { name: '显示名称', exact: true }).fill('物品新版本')
  await edit.getByRole('button', { name: '保存定义', exact: true }).click()
  await expect(edit).not.toBeVisible()
  await expect(row).toContainText('物品新版本')
  await row.getByRole('button', { name: '删除', exact: true }).click()
  await page
    .getByRole('dialog', { name: '删除生成定义', exact: true })
    .getByRole('button', { name: '删除', exact: true })
    .click()
  await expect(row).toHaveCount(0)
  const history = page.locator('.codegen-history').getByRole('row').filter({ hasText: result.id })
  await expect(history).toContainText('生成物品')
  await history.getByRole('button', { name: '预览', exact: true }).click()
  await expect(preview).toBeVisible()
  await expect(preview.getByRole('textbox', { name: '源码预览', exact: true })).toHaveValue(
    /独立工程/,
  )
  await page.screenshot({
    path: '.superpowers/sdd/2026-10-24-tri-admin-greenfield/codegen-ui.png',
    fullPage: true,
    animations: 'disabled',
  })
})
test('lost generation acknowledgement is queried without rerunning and read only users cannot mutate', async ({
  page,
  browser,
}) => {
  const account = await login(page)
  await workspace(page)
  const resource = 'recover_' + randomUUID().replaceAll('-', '').slice(0, 12),
    row = await create(page, resource)
  let writes = 0,
    finished = false,
    request = ''
  await page.route('**/api/v1/codegen/generations', async (route) => {
    if (route.request().method() !== 'POST') return route.continue()
    writes++
    request = route.request().postDataJSON().requestId
    await route.fetch()
    await route.abort()
    finished = true
  })
  await row.getByRole('button', { name: '生成工程', exact: true }).click()
  await expect.poll(() => finished).toBe(true)
  await expect(page.getByText('待确认生成操作', { exact: true })).toBeVisible()
  expect(writes).toBe(1)
  const stored = await page.evaluate(() =>
    Object.entries(localStorage)
      .filter(([key]) => key.startsWith('tri-admin.codegen.'))
      .map(([, value]) => value)
      .join(''),
  )
  expect(stored).toContain(request)
  expect(stored).not.toContain('生成物品')
  expect(stored).not.toContain('"fields"')
  await page.unrouteAll({ behavior: 'wait' })
  await page.reload()
  await login(page, undefined, account)
  await workspace(page)
  await page.getByRole('button', { name: '恢复查询', exact: true }).click()
  await expect(page.getByText('待确认生成操作', { exact: true })).not.toBeVisible()
  expect(writes).toBe(1)
  const context = await browser.newContext(),
    readonly = await context.newPage()
  try {
    await login(readonly, ['codegen.read'])
    await workspace(readonly)
    await expect(readonly.getByRole('button', { name: '新建定义', exact: true })).toHaveCount(0)
    const item = readonly
      .locator('.codegen-definitions')
      .getByRole('row')
      .filter({ hasText: resource })
    await expect(item).toBeVisible()
    for (const name of ['编辑', '删除', '生成工程'])
      await expect(item.getByRole('button', { name, exact: true })).toHaveCount(0)
    await expect(
      readonly
        .locator('.codegen-history')
        .getByRole('button', { name: '预览', exact: true })
        .first(),
    ).toBeVisible()
  } finally {
    await context.close()
  }
})

test('a stale definition requires explicit reload and corrupt local recovery can be cleared without server writes', async ({
  page,
}) => {
  const account = await login(page)
  const resource = 'clear_' + randomUUID().replaceAll('-', '').slice(0, 12),
    schema = {
      resource,
      label: '原始定义',
      fields: [
        {
          name: 'name',
          label: '名称',
          kind: 'text',
          required: true,
          searchable: true,
          unique: false,
        },
      ],
    }
  const response = await page.request.post(account.api + '/codegen/definitions', {
    headers: account.headers,
    data: { requestId: randomUUID(), schema },
  })
  expect(response.status()).toBe(201)
  const rowData = (await response.json()).data
  await workspace(page)
  const row = page.locator('.codegen-definitions').getByRole('row').filter({ hasText: resource })
  await row.getByRole('button', { name: '编辑', exact: true }).click()
  const form = page.getByRole('dialog', { name: '编辑生成定义', exact: true })
  expect(
    (
      await page.request.patch(`${account.api}/codegen/definitions/${rowData.id}`, {
        headers: account.headers,
        data: { version: 1, schema: { ...schema, label: '后台并发更新' } },
      })
    ).status(),
  ).toBe(200)
  await form.getByRole('textbox', { name: '显示名称', exact: true }).fill('不应覆盖')
  await form.getByRole('button', { name: '保存定义', exact: true }).click()
  await expect(form.getByRole('alert')).toContainText('版本已改变')
  await form.getByRole('button', { name: '重新读取', exact: true }).click()
  await expect(form.getByRole('textbox', { name: '显示名称', exact: true })).toHaveValue(
    '后台并发更新',
  )
  await form.getByRole('textbox', { name: '显示名称', exact: true }).fill('已核对的新名称')
  await form.getByRole('button', { name: '保存定义', exact: true }).click()
  await expect(form).not.toBeVisible()
  await expect(row).toContainText('已核对的新名称')
  await page.evaluate(async () => {
    const storePath = '/src/stores/session.ts',
      modelPath = '/src/core/codegen.ts'
    const { useSession } = (await import(
      /* @vite-ignore */ storePath
    )) as typeof import('../../apps/desktop/src/stores/session')
    const { CodegenRecoveryStore } = (await import(
      /* @vite-ignore */ modelPath
    )) as typeof import('../../apps/desktop/src/core/codegen')
    const session = useSession()
    if (!session.user) throw new Error('Fixture identity missing')
    const store = new CodegenRecoveryStore(
      localStorage,
      session.settings.apiBaseUrl,
      session.user.id,
    )
    store.put({
      requestId: crypto.randomUUID(),
      kind: 'definition.create',
      definitionId: null,
      definitionVersion: null,
      createdAt: new Date().toISOString(),
    })
    const key = Object.keys(localStorage).find(
      (key) => key.startsWith('tri-admin.codegen.') && key.endsWith(':' + session.user!.id),
    )
    if (!key) throw new Error('Recovery fixture missing')
    localStorage.setItem(key, '{"broken":true}')
  })
  await page.reload()
  await login(page, undefined, account)
  await workspace(page)
  await expect(page.getByRole('button', { name: '新建定义', exact: true })).toBeDisabled()
  await page.getByRole('button', { name: '清除本机记录', exact: true }).click()
  await page
    .getByRole('dialog', { name: '清除本机恢复记录', exact: true })
    .getByRole('button', { name: '清除本机记录', exact: true })
    .click()
  await expect(page.getByRole('button', { name: '新建定义', exact: true })).toBeEnabled()
  await expect(row).toContainText('已核对的新名称')
})
