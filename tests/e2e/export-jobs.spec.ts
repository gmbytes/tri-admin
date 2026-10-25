import { test, expect, type Page } from '@playwright/test'
import { readFile } from 'node:fs/promises'
async function login(page: Page) {
  const api = process.env.TRI_E2E_API!,
    secret = await readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8'),
    username = 'job-' + Date.now().toString(36)
  expect(
    (
      await page.request.post(api + '/users', {
        headers: { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` },
        data: {
          username,
          displayName: '导出任务验证',
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
  return { api, username, secret }
}
test('background export creates a durable task and downloads verified real data', async ({
  page,
}) => {
  test.setTimeout(60000)
  const { api } = await login(page),
    prefix = 'ui.export.' + Date.now().toString(36),
    headers = { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` }
  for (let index = 0; index < 3; index++)
    expect(
      (
        await page.request.post(api + '/parameters', {
          headers,
          data: {
            name: '后台导出',
            key: prefix + '.' + index,
            valueType: 'boolean',
            value: index === 0,
          },
        })
      ).status(),
    ).toBe(201)
  await page.getByRole('textbox', { name: '导出筛选', exact: true }).fill(prefix)
  const panel = page.locator('.export-jobs-panel')
  const limit = panel.getByRole('spinbutton', { name: '后台行数上限', exact: true })
  await limit.fill('')
  await expect(panel.getByRole('button', { name: '创建后台任务', exact: true })).toBeDisabled()
  await limit.fill('100000')
  await panel.getByRole('button', { name: '创建后台任务', exact: true }).click()
  const row = panel
    .getByRole('row')
    .filter({ has: page.getByText('已完成', { exact: true }) })
    .first()
  await expect(row).toBeVisible({ timeout: 20000 })
  await expect(row).toContainText('3 / 100000')
  await expect(panel.locator('.job-detail')).toContainText('已完成')
  const event = page.waitForEvent('download')
  await row.getByRole('button', { name: '下载结果', exact: true }).click()
  const file = await event
  const bytes = await readFile((await file.path())!, 'utf8')
  expect(bytes).toContain(prefix + '.0')
  expect(bytes).toContain('true')
  expect(bytes).toContain('false')
  await expect(panel.getByRole('button', { name: '恢复查询', exact: true })).toHaveCount(0)
  await panel.screenshot({
    path: '.superpowers/sdd/2026-10-24-tri-admin-greenfield/export-jobs-panel.png',
    animations: 'disabled',
  })
})
test('lost task creation response is recovered after reload using only its saved request identifier', async ({
  page,
}) => {
  const { username, secret } = await login(page)
  let writes = 0,
    requestId = ''
  await page.route('**/api/v1/data-tools/exports', async (route) => {
    if (route.request().method() !== 'POST') return route.continue()
    writes++
    requestId = route.request().postDataJSON().requestId
    const response = await route.fetch()
    expect(response.status()).toBe(202)
    await route.abort('failed')
  })
  const panel = page.locator('.export-jobs-panel')
  await panel.getByRole('button', { name: '创建后台任务', exact: true }).click()
  await expect(panel).toContainText('待确认操作')
  const stored = await page.evaluate(() =>
    Object.entries(localStorage)
      .filter(([key]) => key.startsWith('tri-admin.export-operations.v1:'))
      .map(([, value]) => value)
      .join(),
  )
  expect(stored).toContain(requestId)
  expect(stored).not.toMatch(/content|filter|token|password|path/i)
  await page.reload()
  await page.getByRole('textbox', { name: '账号', exact: true }).fill(username)
  await page.getByLabel('密码', { exact: true }).fill(secret)
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await page.locator('nav').getByRole('button', { name: '数据工具', exact: true }).click()
  await page
    .locator('.export-jobs-panel')
    .getByRole('button', { name: '恢复查询', exact: true })
    .click()
  await expect(
    page.locator('.export-jobs-panel').getByRole('button', { name: '恢复查询', exact: true }),
  ).toHaveCount(0)
  expect(writes).toBe(1)
})

test('a lost retry acknowledgement is queried by operation receipt without repeating retry', async ({
  page,
}) => {
  const { api } = await login(page),
    prefix = 'ui.retry.' + Date.now().toString(36),
    headers = { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` }
  for (let index = 0; index < 2; index++)
    expect(
      (
        await page.request.post(api + '/parameters', {
          headers,
          data: { name: '重试验证', key: prefix + '.' + index, valueType: 'boolean', value: true },
        })
      ).status(),
    ).toBe(201)
  await page.getByRole('textbox', { name: '导出筛选', exact: true }).fill(prefix)
  const panel = page.locator('.export-jobs-panel')
  await panel.getByRole('spinbutton', { name: '后台行数上限', exact: true }).fill('1')
  const creation = page.waitForResponse(
    (response) =>
      response.url() === api + '/data-tools/exports' && response.request().method() === 'POST',
  )
  await panel.getByRole('button', { name: '创建后台任务', exact: true }).click()
  const job = (await (await creation).json()).data
  const row = panel.getByRole('row').filter({ hasText: job.requestId })
  await expect(row).toContainText('失败', { timeout: 20000 })
  let retries = 0,
    operation = ''
  await page.route(`**/data-tools/exports/${job.id}/retry`, async (route) => {
    retries++
    operation = route.request().postDataJSON().requestId
    const response = await route.fetch()
    expect(response.status()).toBe(202)
    await route.abort('failed')
  })
  await row.getByRole('button', { name: '重试任务', exact: true }).click()
  await page
    .getByRole('dialog', { name: '重试导出', exact: true })
    .getByRole('button', { name: '确定', exact: true })
    .click()
  await expect(panel).toContainText('待确认操作')
  const receipt = page.waitForResponse((response) =>
    response.url().endsWith(`/actions/${operation}`),
  )
  await panel.getByRole('button', { name: '恢复查询', exact: true }).click()
  expect((await (await receipt).json()).data.requestId).toBe(operation)
  await expect(panel.getByRole('button', { name: '恢复查询', exact: true })).toHaveCount(0)
  expect(retries).toBe(1)
})

test('read-only task owners can download their existing result without write controls', async ({
  page,
}) => {
  const api = process.env.TRI_E2E_API!,
    secret = await readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8'),
    suffix = Date.now().toString(36),
    headers = { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` },
    username = 'job-reader-' + suffix
  const roleResponse = await page.request.post(api + '/roles', {
    headers,
    data: {
      name: username,
      slug: username,
      permissions: ['exports.read', 'exports.write', 'parameters.read'],
      dataScope: 'self',
    },
  })
  expect(roleResponse.status()).toBe(201)
  const role = (await roleResponse.json()).data
  expect(
    (
      await page.request.post(api + '/users', {
        headers,
        data: { username, displayName: '导出只读', password: secret, roleIds: [role.id] },
      })
    ).status(),
  ).toBe(201)
  await page.addInitScript((endpoint) => localStorage.setItem('tri-admin.server', endpoint), api)
  await page.goto('/#/login')
  await page.getByRole('textbox', { name: '账号', exact: true }).fill(username)
  await page.getByLabel('密码', { exact: true }).fill(secret)
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await page.locator('nav').getByRole('button', { name: '数据工具', exact: true }).click()
  const panel = page.locator('.export-jobs-panel')
  await panel.getByRole('button', { name: '创建后台任务', exact: true }).click()
  await expect(panel.getByText('已完成', { exact: true })).toBeVisible({ timeout: 20000 })
  expect(
    (
      await page.request.patch(api + '/roles/' + role.id, {
        headers,
        data: { version: 1, permissions: ['exports.read', 'parameters.read'] },
      })
    ).status(),
  ).toBe(200)
  await page.reload()
  await page.getByRole('textbox', { name: '账号', exact: true }).fill(username)
  await page.getByLabel('密码', { exact: true }).fill(secret)
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await page.locator('nav').getByRole('button', { name: '数据工具', exact: true }).click()
  await expect(panel.getByRole('button', { name: '创建后台任务', exact: true })).toHaveCount(0)
  for (const name of ['取消任务', '重试任务'])
    await expect(panel.getByRole('button', { name, exact: true })).toHaveCount(0)
  const event = page.waitForEvent('download')
  await panel.getByRole('button', { name: '下载结果', exact: true }).click()
  expect((await event).suggestedFilename()).toMatch(/^tri-admin-parameters-/)
})

test('task paging keeps the latest page when an older refresh response arrives late', async ({
  page,
}) => {
  const { api, username, secret } = await login(page),
    auth = (
      await (
        await page.request.post(api + '/auth/login', { data: { username, password: secret } })
      ).json()
    ).data,
    headers = { authorization: `Bearer ${auth.accessToken}` },
    created: string[] = []
  try {
    for (let index = 0; index < 23; index++) {
      const response = await page.request.post(api + '/data-tools/exports', {
        headers,
        data: {
          requestId: crypto.randomUUID(),
          format: 'csv',
          maxRows: 10,
          source: {
            kind: 'resource',
            resource: 'parameters',
            columns: [{ field: 'key', header: 'key' }],
            search: 'no-page-fixture-' + Date.now(),
          },
        },
      })
      expect(response.status()).toBe(202)
      const job = (await response.json()).data
      created.push(job.requestId)
      const cancelled = await page.request.post(`${api}/data-tools/exports/${job.id}/cancel`, {
        headers,
        data: {},
      })
      expect([200, 409]).toContain(cancelled.status())
    }
    const panel = page.locator('.export-jobs-panel')
    await panel.getByRole('button', { name: '刷新任务', exact: true }).click()
    await expect(panel.locator('.el-table__body tbody tr')).toHaveCount(20)
    let release!: () => void,
      finished!: () => void,
      held = false
    const gate = new Promise<void>((resolve) => {
        release = resolve
      }),
      done = new Promise<void>((resolve) => {
        finished = resolve
      })
    await page.route('**/data-tools/exports?*', async (route) => {
      const url = new URL(route.request().url())
      if (!held && url.searchParams.get('page') === '1') {
        held = true
        const response = await route.fetch()
        await gate
        await route.fulfill({ response }).catch(() => undefined)
        finished()
      } else await route.continue()
    })
    const old = page.waitForRequest(
      (request) =>
        new URL(request.url()).pathname.endsWith('/data-tools/exports') &&
        new URL(request.url()).searchParams.get('page') === '1',
    )
    await panel.getByRole('button', { name: '刷新任务', exact: true }).click()
    await old
    await panel.locator('.el-pagination .btn-next').click()
    await expect(panel.locator('.el-table__body tbody tr')).toHaveCount(3)
    for (const request of created.slice(0, 3))
      await expect(panel.getByText(request, { exact: true })).toBeVisible()
    release()
    await done
    await expect(panel.locator('.el-table__body tbody tr')).toHaveCount(3)
  } finally {
    await page.request.post(api + '/auth/logout', {
      headers,
      data: { refreshToken: auth.refreshToken },
    })
  }
})

test('background tasks execute the selected named template headers and saved filters', async ({
  page,
}) => {
  const { api } = await login(page),
    suffix = Date.now().toString(36),
    prefix = 'ui.templatejob.' + suffix,
    code = 'ui.templatejob.' + suffix,
    headers = { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` }
  for (const group of ['take', 'skip'])
    expect(
      (
        await page.request.post(api + '/parameters', {
          headers,
          data: {
            name: '后台模板',
            key: prefix + '.' + group,
            group,
            valueType: 'boolean',
            value: group === 'take',
          },
        })
      ).status(),
    ).toBe(201)
  const created = await page.request.post(api + '/export-templates', {
    headers,
    data: {
      name: '后台模板',
      code,
      resource: 'parameters',
      columns: [
        { field: 'key', header: '数据标识' },
        { field: 'value', header: '内容' },
      ],
      search: prefix,
      filters: [{ field: 'group', op: 'eq', value: 'take' }],
    },
  })
  expect(created.status()).toBe(201)
  const templates = page.locator('.export-templates-panel')
  await templates.getByRole('button', { name: '查询模板', exact: true }).click()
  await templates
    .getByRole('row')
    .filter({ hasText: code })
    .getByRole('button', { name: '使用模板', exact: true })
    .click()
  await expect(page.locator('.active-export-template')).toContainText('后台模板')
  const panel = page.locator('.export-jobs-panel')
  await panel.getByRole('button', { name: '创建后台任务', exact: true }).click()
  const row = panel.getByRole('row').filter({ hasText: '后台模板' })
  await expect(row).toContainText('已完成', { timeout: 20000 })
  const event = page.waitForEvent('download')
  await row.getByRole('button', { name: '下载结果', exact: true }).click()
  const file = await event,
    bytes = await readFile((await file.path())!, 'utf8')
  expect(bytes).toContain('数据标识,内容')
  expect(bytes).toContain(prefix + '.take')
  expect(bytes).not.toContain(prefix + '.skip')
})

test('cancellation requires confirmation and a lost acknowledgement is resolved by query', async ({
  page,
}) => {
  const api = process.env.TRI_E2E_API!,
    secret = await readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8')
  await page.addInitScript((endpoint) => localStorage.setItem('tri-admin.server', endpoint), api)
  await page.goto('/#/login')
  await page.getByRole('textbox', { name: '账号', exact: true }).fill('export-controls-fixture')
  await page.getByLabel('密码', { exact: true }).fill(secret)
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await page.locator('nav').getByRole('button', { name: '数据工具', exact: true }).click()
  const panel = page.locator('.export-jobs-panel')
  await expect(panel.getByText('排队中', { exact: true })).toBeVisible()
  let writes = 0
  await page.route('**/data-tools/exports/*/cancel', async (route) => {
    writes++
    const response = await route.fetch()
    expect(response.status()).toBe(200)
    await route.abort('failed')
  })
  await panel.getByRole('button', { name: '取消任务', exact: true }).click()
  let dialog = page.getByRole('dialog', { name: '取消导出', exact: true })
  await dialog.getByRole('button', { name: '取消', exact: true }).click()
  expect(writes).toBe(0)
  await panel.getByRole('button', { name: '取消任务', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '取消导出', exact: true })
  await dialog.getByRole('button', { name: '确定', exact: true }).click()
  await expect(panel).toContainText('待确认操作')
  await panel.getByRole('button', { name: '恢复查询', exact: true }).click()
  await expect(panel.getByRole('button', { name: '恢复查询', exact: true })).toHaveCount(0)
  await expect(panel.getByText('已取消', { exact: true })).toBeVisible()
  expect(writes).toBe(1)
})
