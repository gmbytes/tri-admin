import { test, expect, type Page } from '@playwright/test'
import { readFile } from 'node:fs/promises'
async function login(page: Page) {
  const api = process.env.TRI_E2E_API!,
    secret = await readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8'),
    username = 'tools-' + Date.now().toString(36)
  expect(
    (
      await page.request.post(api + '/users', {
        headers: { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` },
        data: {
          username,
          displayName: '数据工具验证',
          password: secret,
          roleIds: [process.env.TRI_E2E_OWNER_ROLE],
        },
      })
    ).status(),
  ).toBe(201)
  await page.addInitScript((endpoint) => localStorage.setItem('tri-admin.server', endpoint), api)
  const auth = await signIn(page, username, secret)
  return { api, username, secret, accessToken: auth.accessToken }
}
async function signIn(page: Page, username: string, secret: string) {
  await page.goto('/#/login')
  await page.getByRole('textbox', { name: '账号', exact: true }).fill(username)
  await page.getByLabel('密码', { exact: true }).fill(secret)
  const response = page.waitForResponse(
    (reply) =>
      reply.url() === process.env.TRI_E2E_API! + '/auth/login' &&
      reply.request().method() === 'POST',
  )
  await page.getByRole('button', { name: '登录', exact: true }).click()
  const reply = await response
  expect(reply.status()).toBe(200)
  const auth = (await reply.json()).data as { accessToken: string }
  await expect(page.locator('.sidebar')).toBeVisible()
  await page.locator('nav').getByRole('button', { name: '数据工具', exact: true }).click()
  await expect(page.getByRole('heading', { name: '数据工具', exact: true })).toBeVisible()
  return auth
}
async function mapColumn(page: Page, field: string, header: string) {
  const control = page.getByRole('combobox', { name: field + ' 来源列', exact: true })
  const row = page
    .locator('.data-field-mapping-row')
    .filter({ has: page.getByRole('combobox', { name: field + ' 来源列', exact: true }) })
  await row.locator('.el-select__wrapper').click()
  const listbox = await control.getAttribute('aria-controls')
  if (!listbox) throw new Error('Missing mapping listbox')
  await page.locator(`[id="${listbox}"]`).getByRole('option', { name: header, exact: true }).click()
}
test('real CSV mapping preview applies valid rows once and XLSX exports can be reimported', async ({
  page,
}) => {
  test.setTimeout(60000)
  const { api, username, secret } = await login(page),
    suffix = Date.now().toString(36),
    key = 'tools.csv.' + suffix
  await page.locator('input[type=file]').setInputFiles({
    name: 'import.csv',
    mimeType: 'text/csv',
    buffer: Buffer.from(
      `名称,标识,类型,值\n布尔参数,${key},boolean,true\n错误参数,tools.invalid.${suffix},boolean,maybe\n嵌套参数,tools.deep.${suffix},json,${'['.repeat(16)}null${']'.repeat(16)}\n`,
    ),
  })
  await expect(page.getByRole('combobox', { name: '名称 来源列', exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: '生成导入预览', exact: true })).toBeDisabled()
  await mapColumn(page, '名称', '名称')
  await mapColumn(page, '标识', '标识')
  await mapColumn(page, '值类型', '类型')
  await mapColumn(page, '值', '值')
  await expect(page.getByRole('textbox', { name: '字段映射', exact: true })).toHaveCount(0)
  await page.getByRole('heading', { name: '数据工具', exact: true }).scrollIntoViewIfNeeded()
  await page.screenshot({
    path: '.superpowers/sdd/2026-10-24-tri-admin-greenfield/data-mapping-selector.png',
    fullPage: true,
    animations: 'disabled',
  })
  await page.getByRole('button', { name: '生成导入预览', exact: true }).click()
  await expect(page.locator('.data-import-results').getByText(key, { exact: true })).toBeVisible()
  await expect(
    page.locator('.data-import-results').getByText('tools.deep.' + suffix, { exact: true }),
  ).toBeVisible()
  await expect(
    page.locator('.data-import-results').getByText('字段无效', { exact: true }),
  ).toBeVisible()
  await page.getByRole('button', { name: '应用可导入行', exact: true }).click()
  await expect(
    page.locator('.data-import-results').getByText('已创建', { exact: true }),
  ).toHaveCount(2)
  await page.getByRole('button', { name: '查询最新结果', exact: true }).click()
  await expect(
    page.locator('.data-import-results').getByText('已创建', { exact: true }),
  ).toHaveCount(2)
  await page.getByPlaceholder('按资源标识前缀筛选').fill(key)
  await page
    .locator('.el-form-item')
    .filter({ hasText: '表格格式' })
    .locator('.el-select__wrapper')
    .click()
  await page.getByRole('option', { name: 'XLSX', exact: true }).click()
  await expect(page.locator('.data-import-results').getByText(key, { exact: true })).toBeVisible()
  const downloadEvent = page.waitForEvent('download')
  await page.getByRole('button', { name: '导出数据', exact: true }).click()
  const download = await downloadEvent
  const path = await download.path()
  if (!path) throw new Error('Missing downloaded workbook')
  const workbook = await readFile(path)
  expect(workbook.subarray(0, 2).toString()).toBe('PK')
  // Use the authenticated fixture identity to verify the resource and remove only this fixture.
  const headers = { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` }
  const resource = await page.request.get(api + '/parameters?search=' + key, { headers })
  const rows = (await resource.json()).data
  expect(rows).toHaveLength(1)
  expect(rows[0].value).toBe(true)
  expect(
    (
      await page.request.delete(`${api}/parameters/${rows[0].id}?version=${rows[0].version}`, {
        headers,
      })
    ).status(),
  ).toBe(204)
  await page.locator('input[type=file]').setInputFiles({
    name: 'roundtrip.xlsx',
    mimeType: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
    buffer: workbook,
  })
  await page.getByRole('button', { name: '生成导入预览', exact: true }).click()
  await expect(page.getByRole('button', { name: '应用可导入行', exact: true })).toBeEnabled()
  await page.getByRole('button', { name: '应用可导入行', exact: true }).click()
  await expect(
    page.locator('.data-import-results').getByText('已创建', { exact: true }),
  ).toBeVisible()
  const final = (
    await (await page.request.get(api + '/parameters?search=' + key, { headers })).json()
  ).data
  expect(final).toHaveLength(1)
  expect(final[0].value).toBe(true)
  await page.reload()
  await signIn(page, username, secret)
  await expect(page.getByRole('heading', { name: '数据工具', exact: true })).toBeVisible()
  await page
    .locator('.data-import-history')
    .getByRole('button', { name: '查看结果' })
    .first()
    .click()
  await expect(
    page.locator('.data-import-results').getByText('已创建', { exact: true }),
  ).toBeVisible()
  await page.getByRole('heading', { name: '数据工具', exact: true }).scrollIntoViewIfNeeded()
  await page.screenshot({
    animations: 'disabled',
    path: '.superpowers/sdd/2026-10-24-tri-admin-greenfield/data-tools-workspace.png',
    fullPage: true,
  })
})

test('a lost apply response is reconciled by query without another write', async ({ page }) => {
  const { api } = await login(page),
    key = 'tools.lost.' + Date.now().toString(36)
  await page.locator('input[type=file]').setInputFiles({
    name: 'lost.csv',
    mimeType: 'text/csv',
    buffer: Buffer.from(`name,key,valueType,value\nLost response,${key},string,value\n`),
  })
  await page.getByRole('button', { name: '生成导入预览', exact: true }).click()
  await expect(page.getByRole('button', { name: '应用可导入行', exact: true })).toBeEnabled()
  let writes = 0
  await page.route('**/data-tools/imports/*/apply', async (route) => {
    writes++
    await route.fetch()
    await route.abort('failed')
  })
  await page.getByRole('button', { name: '应用可导入行', exact: true }).click()
  await expect(page.locator('.data-alert')).toBeVisible()
  await page.getByRole('button', { name: '查询最新结果', exact: true }).click()
  await expect(
    page.locator('.data-import-results').getByText('已创建', { exact: true }),
  ).toBeVisible()
  await expect(page.getByRole('button', { name: '应用可导入行', exact: true })).toBeDisabled()
  expect(writes).toBe(1)
  const response = await page.request.get(api + '/parameters?search=' + key, {
    headers: { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` },
  })
  expect((await response.json()).data).toHaveLength(1)
})
async function chooseResource(page: Page, label: string) {
  await page
    .locator('.el-form-item')
    .filter({ hasText: '数据资源' })
    .locator('.el-select__wrapper')
    .click()
  await page.getByRole('option', { name: label, exact: true }).click()
}
test('dictionary and typed item tools persist real CSV and XLSX data with visible field previews', async ({
  page,
}) => {
  const { api } = await login(page),
    key = 'ui.dictionary.' + Date.now().toString(36),
    headers = { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` }
  await chooseResource(page, '字典')
  await page.locator('input[type=file]').setInputFiles({
    name: 'dictionary.csv',
    mimeType: 'text/csv',
    buffer: Buffer.from(`name,key,description,enabled\n页面字典,${key},测试描述,false\n`),
  })
  await page.getByRole('button', { name: '生成导入预览', exact: true }).click()
  await page.getByRole('button', { name: '应用可导入行', exact: true }).click()
  await expect(
    page.locator('.data-import-results').getByText('已创建', { exact: true }),
  ).toBeVisible()
  const parent = (
    await (await page.request.get(api + '/dictionaries?search=' + key, { headers })).json()
  ).data[0]
  expect(parent.enabled).toBe(false)
  const source = await page.request.post(`${api}/dictionaries/${parent.id}/items`, {
    headers,
    data: { label: '明细源', value: '001', sort: -3, enabled: false },
  })
  expect(source.status()).toBe(201)
  const item = (await source.json()).data
  const exported = await page.request.post(api + '/data-tools/export', {
    headers,
    data: { resource: 'dictionaryItems', format: 'xlsx', search: key },
  })
  expect(exported.status()).toBe(200)
  const file = (await exported.json()).data
  expect(
    (
      await page.request.delete(`${api}/dictionaries/${parent.id}/items/${item.id}?version=1`, {
        headers,
      })
    ).status(),
  ).toBe(204)
  await chooseResource(page, '字典明细')
  await page.locator('input[type=file]').setInputFiles({
    name: 'items.xlsx',
    mimeType: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
    buffer: Buffer.from(file.contentBase64, 'base64'),
  })
  await page.getByRole('button', { name: '生成导入预览', exact: true }).click()
  const row = page
    .locator('.data-import-results .el-table__body-wrapper')
    .getByRole('row')
    .filter({ hasText: key + ' / 001' })
  await row.locator('.el-table__expand-icon').click()
  await expect(page.locator('.data-row-details').getByText('-3', { exact: true })).toBeVisible()
  await expect(page.locator('.data-row-details').getByText('false', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: '应用可导入行', exact: true }).click()
  await expect(
    page.locator('.data-import-results').getByText('已创建', { exact: true }),
  ).toBeVisible()
  const saved = (
    await (await page.request.get(`${api}/dictionaries/${parent.id}/items`, { headers })).json()
  ).data
  expect(saved).toHaveLength(1)
  expect(saved[0]).toMatchObject({ value: '001', sort: -3, enabled: false })
  await page
    .locator('.el-form-item')
    .filter({ hasText: '表格格式' })
    .locator('.el-select__wrapper')
    .click()
  await page.getByRole('option', { name: 'XLSX', exact: true }).click()
  const downloadEvent = page.waitForEvent('download')
  await page.getByRole('button', { name: '导出数据', exact: true }).click()
  const download = await downloadEvent
  const path = await download.path()
  expect((await readFile(path!)).subarray(0, 2).toString()).toBe('PK')
})

test('inspection integrity failures keep imports blocked and can be retried without reselecting bytes', async ({
  page,
}) => {
  await login(page)
  let inspections = 0,
    creates = 0
  page.on('request', (request) => {
    if (request.url().endsWith('/data-tools/imports') && request.method() === 'POST') creates++
  })
  await page.route('**/data-tools/inspect', async (route) => {
    inspections++
    const response = await route.fetch()
    const body = await response.json()
    body.data.sha256 = '0'.repeat(64)
    await route.fulfill({ response, json: body })
  })
  await page.locator('input[type=file]').setInputFiles({
    name: 'retry.csv',
    mimeType: 'text/csv',
    buffer: Buffer.from('name,key,valueType,value\nRetry,inspect.retry,string,value\n'),
  })
  await expect(page.locator('.data-alert')).toContainText('内容摘要校验失败')
  await expect(page.getByRole('button', { name: '生成导入预览', exact: true })).toBeDisabled()
  expect(creates).toBe(0)
  await page.unroute('**/data-tools/inspect')
  await page.getByRole('button', { name: '重试读取表头', exact: true }).click()
  await expect(page.getByRole('button', { name: '生成导入预览', exact: true })).toBeEnabled()
  expect(inspections).toBe(1)
  await page.getByRole('button', { name: '生成导入预览', exact: true }).click()
  await expect(
    page.locator('.data-import-results').getByText('inspect.retry', { exact: true }),
  ).toBeVisible()
  expect(creates).toBe(1)
})
test('an item preview reports a parent rename instead of writing into a replacement dictionary', async ({
  page,
}) => {
  const { api } = await login(page),
    key = 'ui.bound.' + Date.now().toString(36),
    headers = { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` }
  const response = await page.request.post(api + '/dictionaries', {
    headers,
    data: { name: 'Original', key },
  })
  expect(response.status()).toBe(201)
  const parent = (await response.json()).data
  await chooseResource(page, '字典明细')
  await page.locator('input[type=file]').setInputFiles({
    name: 'bound.csv',
    mimeType: 'text/csv',
    buffer: Buffer.from(`dictionaryKey,label,value\n${key},Bound,item\n`),
  })
  await page.getByRole('button', { name: '生成导入预览', exact: true }).click()
  await expect(page.getByRole('button', { name: '应用可导入行', exact: true })).toBeEnabled()
  expect(
    (
      await page.request.patch(`${api}/dictionaries/${parent.id}`, {
        headers,
        data: { version: 1, key: key + '.renamed' },
      })
    ).status(),
  ).toBe(200)
  expect(
    (
      await page.request.post(api + '/dictionaries', {
        headers,
        data: { name: 'Replacement', key },
      })
    ).status(),
  ).toBe(201)
  await page.getByRole('button', { name: '应用可导入行', exact: true }).click()
  await expect(
    page.locator('.data-import-results').getByText('父字典已改变，请重新预览', { exact: true }),
  ).toBeVisible()
  await expect(page.getByRole('button', { name: '应用可导入行', exact: true })).toBeDisabled()
  const remaining = (
    await (await page.request.get(`${api}/dictionaries/${parent.id}/items`, { headers })).json()
  ).data
  expect(remaining).toHaveLength(0)
})

test('cancelled inspection results cannot replace a newly selected file', async ({ page }) => {
  await login(page)
  let release!: () => void,
    completed!: () => void,
    seen = false,
    calls = 0,
    creates = 0
  const gate = new Promise<void>((resolve) => (release = resolve)),
    done = new Promise<void>((resolve) => (completed = resolve))
  page.on('request', (request) => {
    if (request.url().endsWith('/data-tools/imports') && request.method() === 'POST') creates++
  })
  await page.route('**/data-tools/inspect', async (route) => {
    if (++calls !== 1) {
      await route.continue()
      return
    }
    const response = await route.fetch()
    seen = true
    await gate
    try {
      await route.fulfill({ response })
    } catch {
      /* The browser may already have retired this cancelled request. */
    } finally {
      completed()
    }
  })
  try {
    await page.locator('input[type=file]').setInputFiles({
      name: 'old.csv',
      mimeType: 'text/csv',
      buffer: Buffer.from('Old A,Old B\na,b\n'),
    })
    await expect.poll(() => seen).toBe(true)
    await page.getByRole('button', { name: '停止当前操作', exact: true }).click()
    await expect(page.getByRole('button', { name: '重试读取表头', exact: true })).toBeEnabled()
    await page.locator('input[type=file]').setInputFiles({
      name: 'new.csv',
      mimeType: 'text/csv',
      buffer: Buffer.from(
        'name,key,valueType,value\nNew,new.first,string,one\nNew,new.second,string,two\n',
      ),
    })
    await expect(page.getByText(/已识别 2 行数据/)).toBeVisible()
    await expect(page.getByRole('button', { name: '生成导入预览', exact: true })).toBeEnabled()
    release()
    await done
    await expect(page.getByText('new.csv', { exact: true })).toBeVisible()
    await expect(page.getByText(/已识别 2 行数据/)).toBeVisible()
    const control = page.getByRole('combobox', { name: '名称 来源列', exact: true })
    await page
      .locator('.data-field-mapping-row')
      .filter({ has: control })
      .locator('.el-select__wrapper')
      .click()
    const id = await control.getAttribute('aria-controls'),
      list = page.locator(`[id="${id}"]`)
    await expect(list.getByRole('option', { name: 'name', exact: true })).toBeVisible()
    await expect(list.getByRole('option', { name: 'Old A', exact: true })).toHaveCount(0)
    expect(creates).toBe(0)
  } finally {
    release()
    await page.unroute('**/data-tools/inspect')
  }
})

test('history paging finds older pending work while a stale refresh is cancelled', async ({
  page,
}) => {
  test.setTimeout(60000)
  const { api, accessToken } = await login(page),
    headers = { authorization: `Bearer ${accessToken}` },
    suffix = Date.now().toString(36)
  async function createJob(resource: string, text: string) {
    const reply = await page.request.post(api + '/data-tools/imports', {
      headers,
      data: {
        requestId: crypto.randomUUID(),
        resource,
        format: 'csv',
        contentBase64: Buffer.from(text).toString('base64'),
      },
    })
    expect(reply.status()).toBe(201)
    return (await reply.json()).data
  }
  const old = await createJob(
    'parameters',
    `name,key,valueType,value\nOld,history.old.${suffix},string,value\n`,
  )
  for (let i = 0; i < 22; i++) {
    const job = await createJob(
      'parameters',
      `name,key,valueType,value\nDone,history.done.${suffix}.${i},string,value\n`,
    )
    expect(
      (
        await page.request.post(`${api}/data-tools/imports/${job.id}/apply`, {
          headers,
          data: { rows: [0] },
        })
      ).status(),
    ).toBe(200)
  }
  await createJob('dictionaries', `name,key\nPending,history.dict.${suffix}\n`)
  await createJob(
    'parameters',
    `name,key,valueType,value\nInvalid,history.invalid.${suffix},boolean,maybe\n`,
  )
  const panel = page.locator('.data-import-history')
  await panel.getByRole('button', { name: '刷新历史', exact: true }).click()
  await expect(panel.getByText('25 条记录', { exact: true })).toBeVisible()
  let release!: () => void,
    completed!: () => void,
    held = false
  const gate = new Promise<void>((resolve) => (release = resolve)),
    done = new Promise<void>((resolve) => (completed = resolve))
  await page.route('**/data-tools/imports?*', async (route) => {
    if (new URL(route.request().url()).searchParams.get('page') !== '1' || held) {
      await route.continue()
      return
    }
    held = true
    const response = await route.fetch()
    await gate
    try {
      await route.fulfill({ response })
    } catch {
      /* Cancelled stale request. */
    } finally {
      completed()
    }
  })
  try {
    await panel.getByRole('button', { name: '刷新历史', exact: true }).click()
    await expect.poll(() => held).toBe(true)
    const second = page.waitForRequest(
      (request) =>
        new URL(request.url()).pathname.endsWith('/data-tools/imports') &&
        new URL(request.url()).searchParams.get('page') === '2',
    )
    await panel.locator('.btn-next').click()
    await second
    await expect(panel.getByText(old.id, { exact: true })).toBeVisible()
    release()
    await done
    await expect(panel.getByText(old.id, { exact: true })).toBeVisible()
    await panel.screenshot({
      path: '.superpowers/sdd/2026-10-24-tri-admin-greenfield/data-import-history.png',
      animations: 'disabled',
    })
  } finally {
    release()
    await page.unroute('**/data-tools/imports?*')
  }
  async function select(name: string, value: string) {
    const control = page.getByRole('combobox', { name, exact: true })
    await panel
      .locator('.el-select')
      .filter({ has: control })
      .locator('.el-select__wrapper')
      .click()
    const id = await control.getAttribute('aria-controls')
    await page.locator(`[id="${id}"]`).getByRole('option', { name: value, exact: true }).click()
  }
  await select('历史状态', '待处理')
  await expect(panel.getByText('2 条记录', { exact: true })).toBeVisible()
  await select('历史资源', '系统参数')
  await expect(panel.getByText('1 条记录', { exact: true })).toBeVisible()
  await expect(panel.getByText(old.id, { exact: true })).toBeVisible()
})

test('redacted history shows retained outcomes without exposing original row values', async ({
  page,
}) => {
  const api = process.env.TRI_E2E_API!,
    secret = await readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8')
  await page.addInitScript((endpoint) => localStorage.setItem('tri-admin.server', endpoint), api)
  await signIn(page, 'retained-history-fixture', secret)
  const history = page.locator('.data-import-history')
  await expect(history.getByText('部分完成', { exact: true })).toBeVisible()
  await expect(history.getByText('内容已清理', { exact: true })).toBeVisible()
  await history.getByRole('button', { name: '查看结果', exact: true }).click()
  const result = page.locator('.data-import-results')
  await expect(result).toContainText('原始内容已清理')
  await expect(result.getByText('已创建', { exact: true })).toBeVisible()
  await expect(result.getByText('已过期', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: '应用可导入行', exact: true })).toBeDisabled()
  expect(await result.textContent()).not.toContain('PRIVATE_RETAINED_SENTINEL')
  expect(await result.textContent()).not.toContain('fixture.retained.first')
  await result.screenshot({
    path: '.superpowers/sdd/2026-10-24-tri-admin-greenfield/data-import-receipt.png',
    animations: 'disabled',
  })
})
