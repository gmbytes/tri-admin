import { test, expect, type Page, type Locator } from '@playwright/test'
import { readFile } from 'node:fs/promises'
async function login(page: Page) {
  const api = process.env.TRI_E2E_API!,
    secret = await readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8'),
    username = 'template-' + Date.now().toString(36)
  expect(
    (
      await page.request.post(api + '/users', {
        headers: { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` },
        data: {
          username,
          displayName: '模板验证',
          password: secret,
          roleIds: [process.env.TRI_E2E_OWNER_ROLE],
        },
      })
    ).status(),
  ).toBe(201)
  await page.addInitScript((endpoint) => localStorage.setItem('tri-admin.server', endpoint), api)
  await page.goto('/#/login')
  await page.getByRole('textbox', { name: '账号', exact: true }).fill(username)
  await page.getByLabel('密码', { exact: true }).fill(secret)
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await page.locator('nav').getByRole('button', { name: '数据工具', exact: true }).click()
  return api
}
async function choose(page: Page, root: Locator, name: string, label: string) {
  const control = page.getByRole('combobox', { name, exact: true })
  await root.locator('.el-select').filter({ has: control }).locator('.el-select__wrapper').click()
  const id = await control.getAttribute('aria-controls')
  await page.locator(`[id="${id}"]`).getByRole('option', { name: label, exact: true }).click()
}
test('named templates save edit copy export and import using the stored columns and filters', async ({
  page,
}) => {
  test.setTimeout(60000)
  const api = await login(page),
    suffix = Date.now().toString(36),
    code = 'ui.template.' + suffix,
    prefix = 'ui.named.' + suffix,
    headers = { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` }
  for (const [key, group, value] of [
    [prefix + '.keep', 'red', true],
    [prefix + '.skip', 'blue', false],
  ])
    expect(
      (
        await page.request.post(api + '/parameters', {
          headers,
          data: { name: '模板数据', key, group, valueType: 'boolean', value },
        })
      ).status(),
    ).toBe(201)
  const panel = page.locator('.export-templates-panel')
  await panel.getByRole('button', { name: '新建模板', exact: true }).click()
  let dialog = page.getByRole('dialog', { name: '新建模板', exact: true })
  await dialog.getByLabel('模板名称', { exact: true }).fill('页面模板')
  await dialog.getByLabel('模板标识', { exact: true }).fill(code)
  await dialog.getByRole('textbox', { name: 'name 表头', exact: true }).fill('名称')
  await dialog.getByRole('textbox', { name: 'key 表头', exact: true }).fill('标识')
  await dialog.getByRole('textbox', { name: 'valueType 表头', exact: true }).fill('类型')
  await dialog.getByRole('textbox', { name: 'value 表头', exact: true }).fill('数据')
  await dialog.locator('label.el-checkbox').filter({ hasText: '允许导入' }).click()
  await expect(dialog.getByRole('checkbox', { name: '允许导入', exact: true })).toBeChecked()
  await dialog.getByLabel('导出标识前缀', { exact: true }).fill(prefix)
  await dialog.getByRole('button', { name: '添加筛选', exact: true }).click()
  await choose(page, dialog, '筛选字段 1', '分组')
  await dialog.getByRole('textbox', { name: '筛选值 1', exact: true }).fill('red')
  const editor = dialog.locator('.el-dialog'),
    bounds = await editor.boundingBox(),
    viewport = page.viewportSize()!
  expect(bounds).not.toBeNull()
  expect(bounds!.y).toBeGreaterThanOrEqual(0)
  expect(bounds!.height).toBeLessThanOrEqual(viewport.height * 0.91)
  await expect(dialog.getByRole('heading', { name: '新建模板', exact: true })).toBeInViewport()
  await expect(dialog.getByRole('button', { name: '保存模板', exact: true })).toBeInViewport()
  await editor.screenshot({
    path: '.superpowers/sdd/2026-10-24-tri-admin-greenfield/export-template-editor.png',
    animations: 'disabled',
  })
  await dialog.getByRole('button', { name: '保存模板', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  let row = panel.getByRole('row').filter({ hasText: code })
  await row.getByRole('button', { name: '使用模板', exact: true }).click()
  await expect(page.locator('.active-export-template')).toContainText('页面模板')
  await page.getByRole('heading', { name: '数据工具', exact: true }).scrollIntoViewIfNeeded()
  await page.screenshot({
    path: '.superpowers/sdd/2026-10-24-tri-admin-greenfield/export-template-workspace.png',
    fullPage: true,
    animations: 'disabled',
  })
  const event = page.waitForEvent('download')
  await page.getByRole('button', { name: '导出数据', exact: true }).click()
  const file = await event,
    path = await file.path(),
    text = await readFile(path!, 'utf8')
  expect(text).toContain('名称,标识,group,description,类型,数据')
  expect(text).toContain(prefix + '.keep')
  expect(text).not.toContain(prefix + '.skip')
  await row.getByRole('button', { name: '复制', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '复制模板', exact: true })
  await dialog.getByLabel('模板名称', { exact: true }).fill('复制模板')
  await dialog.getByLabel('模板标识', { exact: true }).fill(code + '.copy')
  await dialog.getByRole('button', { name: '复制模板', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  row = panel.getByRole('row').filter({ has: page.getByText(code, { exact: true }) })
  await row.getByRole('button', { name: '编辑', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '编辑模板', exact: true })
  await dialog.getByLabel('模板名称', { exact: true }).fill('更新后模板')
  await dialog.getByRole('button', { name: '保存模板', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  const stale = page.waitForResponse(
    (response) =>
      response.request().method() === 'POST' &&
      /\/export-templates\/[a-f0-9]{24}\/export$/.test(response.url()),
  )
  await page.getByRole('button', { name: '导出数据', exact: true }).click()
  expect((await stale).status()).toBe(409)
  await expect(page.locator('.data-alert')).toContainText('模板已改变，请刷新后重试')
  await expect(page.locator('.active-export-template')).toContainText('v1')
  await row.getByRole('button', { name: '使用模板', exact: true }).click()
  await expect(page.locator('.active-export-template')).toContainText('v2')
  await page.locator('input[type=file]').setInputFiles({
    name: 'named.csv',
    mimeType: 'text/csv',
    buffer: Buffer.from(`名称,标识,类型,数据\n模板导入,${prefix}.imported,boolean,false\n`),
  })
  await page.getByRole('button', { name: '生成导入预览', exact: true }).click()
  await expect(page.locator('.data-import-results')).toContainText('更新后模板 · v2')
  await page.getByRole('button', { name: '应用可导入行', exact: true }).click()
  await expect(
    page.locator('.data-import-results').getByText('已创建', { exact: true }),
  ).toBeVisible()
  const record = (
    await (
      await page.request.get(api + '/parameters?search=' + prefix + '.imported', { headers })
    ).json()
  ).data[0]
  expect(record.value).toBe(false)
  await row.getByRole('button', { name: '删除', exact: true }).click()
  await page
    .getByRole('dialog', { name: '删除模板', exact: true })
    .getByRole('button', { name: '删除', exact: true })
    .click()
  await expect(panel.getByText(code, { exact: true })).toHaveCount(0)
  await page
    .locator('.data-import-history')
    .getByRole('button', { name: '查看结果', exact: true })
    .first()
    .click()
  await expect(page.locator('.data-import-results')).toContainText('更新后模板 · v2')
})

test('read-only template users can reuse empty files without management or data-write controls', async ({
  page,
}) => {
  const api = process.env.TRI_E2E_API!,
    secret = await readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8'),
    suffix = Date.now().toString(36),
    headers = { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` }
  const created = await page.request.post(api + '/export-templates', {
    headers,
    data: {
      name: '只读模板',
      code: 'readonly.' + suffix,
      resource: 'parameters',
      columns: [
        { field: 'name', header: '名称' },
        { field: 'key', header: '标识' },
        { field: 'valueType', header: '类型' },
        { field: 'value', header: '数据' },
      ],
      allowImport: true,
    },
  })
  expect(created.status()).toBe(201)
  const role = await page.request.post(api + '/roles', {
    headers,
    data: {
      name: '模板只读',
      slug: 'template-reader-' + suffix,
      permissions: ['exports.read', 'parameters.read'],
      dataScope: 'self',
    },
  })
  expect(role.status()).toBe(201)
  const roleId = (await role.json()).data.id,
    username = 'template-reader-' + suffix
  expect(
    (
      await page.request.post(api + '/users', {
        headers,
        data: { username, displayName: '模板只读', password: secret, roleIds: [roleId] },
      })
    ).status(),
  ).toBe(201)
  await page.addInitScript((endpoint) => localStorage.setItem('tri-admin.server', endpoint), api)
  await page.goto('/#/login')
  await page.getByRole('textbox', { name: '账号', exact: true }).fill(username)
  await page.getByLabel('密码', { exact: true }).fill(secret)
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await page.locator('nav').getByRole('button', { name: '数据工具', exact: true }).click()
  const panel = page.locator('.export-templates-panel'),
    row = panel.getByRole('row').filter({ hasText: 'readonly.' + suffix })
  await expect(panel.getByRole('button', { name: '新建模板', exact: true })).toHaveCount(0)
  for (const name of ['编辑', '复制', '删除'])
    await expect(row.getByRole('button', { name, exact: true })).toHaveCount(0)
  await row.getByRole('button', { name: '使用模板', exact: true }).click()
  await expect(page.getByRole('button', { name: '导出数据', exact: true })).toBeDisabled()
  await expect(page.getByRole('button', { name: '选择数据文件', exact: true })).toHaveCount(0)
  const event = page.waitForEvent('download')
  await page.getByRole('button', { name: '下载空白模板', exact: true }).click()
  const file = await event
  expect(await readFile((await file.path())!, 'utf8')).toContain('名称,标识,类型,数据')
})

test('lost template preview creation can be confirmed after the configuration is deleted', async ({
  page,
}) => {
  const api = await login(page),
    suffix = Date.now().toString(36),
    code = 'rescue.' + suffix,
    key = 'rescue.import.' + suffix,
    headers = { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` }
  const created = await page.request.post(api + '/export-templates', {
    headers,
    data: {
      name: '响应恢复模板',
      code,
      resource: 'parameters',
      columns: [
        { field: 'name', header: 'name' },
        { field: 'key', header: 'key' },
        { field: 'valueType', header: 'valueType' },
        { field: 'value', header: 'value' },
      ],
      allowImport: true,
    },
  })
  expect(created.status()).toBe(201)
  const template = (await created.json()).data
  const panel = page.locator('.export-templates-panel')
  await panel.getByRole('button', { name: '查询模板', exact: true }).click()
  await panel
    .getByRole('row')
    .filter({ hasText: code })
    .getByRole('button', { name: '使用模板', exact: true })
    .click()
  let writes = 0,
    requestId = ''
  await page.route(`**/export-templates/${template.id}/import-previews`, async (route) => {
    writes++
    requestId = route.request().postDataJSON().requestId
    const response = await route.fetch()
    expect(response.status()).toBe(201)
    expect(
      (
        await page.request.delete(`${api}/export-templates/${template.id}?version=1`, { headers })
      ).status(),
    ).toBe(204)
    await route.abort('failed')
  })
  await page.locator('input[type=file]').setInputFiles({
    name: 'rescue.csv',
    mimeType: 'text/csv',
    buffer: Buffer.from(`name,key,valueType,value\nRescue,${key},string,value\n`),
  })
  await page.getByRole('button', { name: '生成导入预览', exact: true }).click()
  await expect(page.locator('.data-alert')).toBeVisible()
  expect(
    (
      await page.request.get(`${api}/data-tools/imports/by-request/${requestId}`, { headers })
    ).status(),
  ).toBe(404)
  await page.getByRole('button', { name: '查询上次预览请求', exact: true }).click()
  await expect(page.locator('.data-import-results')).toContainText('响应恢复模板 · v1')
  await page.getByRole('button', { name: '应用可导入行', exact: true }).click()
  await expect(
    page.locator('.data-import-results').getByText('已创建', { exact: true }),
  ).toBeVisible()
  expect(writes).toBe(1)
  expect(
    (await (await page.request.get(api + '/parameters?search=' + key, { headers })).json()).data,
  ).toHaveLength(1)
})
