import { test, expect, type Page } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { randomUUID, createHash } from 'node:crypto'
async function login(
  page: Page,
  permissions?: string[],
  existing?: { username: string; password: string },
) {
  const api = process.env.TRI_E2E_API!,
    headers = { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` },
    password = existing?.password ?? (await readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8')),
    username = existing?.username ?? 'skills-' + randomUUID().replaceAll('-', '')
  let role = process.env.TRI_E2E_OWNER_ROLE
  if (!existing) {
    if (permissions) {
      const response = await page.request.post(api + '/roles', {
        headers,
        data: {
          name: '扩展权限',
          slug: 'skills-' + randomUUID().replaceAll('-', ''),
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
          data: { username, displayName: '扩展验证', password, roleIds: [role] },
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
  await page.locator('nav').getByRole('button', { name: 'Skills 与插件', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Skills 与插件', exact: true })).toBeVisible()
}
async function create(page: Page, name: string) {
  await page.getByRole('button', { name: '新建 Skill', exact: true }).click()
  const form = page.getByRole('dialog', { name: '新建 Skill', exact: true })
  await form.getByRole('textbox', { name: '名称', exact: true }).fill(name)
  await form.getByRole('textbox', { name: '描述', exact: true }).fill('扩展说明')
  await form
    .getByRole('textbox', { name: 'Markdown 正文', exact: true })
    .fill('# Record helper\nBody content.')
  const response = page.waitForResponse(
    (r) => r.url().endsWith('/api/v1/extensions') && r.request().method() === 'POST',
  )
  await form.getByRole('button', { name: '创建 Skill', exact: true }).click()
  const value = (await (await response).json()).data
  await expect(form).not.toBeVisible()
  return value
}
test('extension workspace edits resources preserves history and exports the original ZIP', async ({
  page,
}) => {
  await page.setViewportSize({ width: 1100, height: 620 })
  await login(page)
  await workspace(page)
  const name = 'record-' + randomUUID().slice(0, 8),
    created = await create(page, name),
    row = page.locator('.extensions-table').getByRole('row').filter({ hasText: name })
  await expect(row).toContainText('停用')
  await row.getByRole('button', { name: '管理文件', exact: true }).click()
  const manager = page.getByRole('dialog', { name: '扩展文件与历史', exact: true })
  await expect(manager.getByRole('textbox', { name: '文件内容', exact: true })).toHaveValue(
    /扩展说明/,
  )
  const box = await manager.boundingBox()
  expect(box!.y + box!.height).toBeLessThanOrEqual(page.viewportSize()!.height - 8)
  await manager.getByRole('button', { name: '新增文件', exact: true }).click()
  const adding = page.getByRole('dialog', { name: '新增扩展文件', exact: true })
  await adding.getByRole('textbox', { name: '文件路径', exact: true }).fill('templates/record.txt')
  await adding.getByRole('textbox', { name: '新增文件内容', exact: true }).fill('模板初版')
  await adding.getByRole('button', { name: '保存文件修订', exact: true }).click()
  await expect(adding).not.toBeVisible()
  await expect(manager.getByRole('textbox', { name: '文件内容', exact: true })).toHaveValue(
    '模板初版',
  )
  await manager.getByRole('textbox', { name: '文件内容', exact: true }).fill('模板修订版')
  await manager.getByRole('button', { name: '保存新修订', exact: true }).click()
  await expect(manager.getByText('修订 3', { exact: true })).toBeVisible()
  const first = manager
    .locator('.extension-revisions')
    .getByRole('row')
    .filter({ hasText: created.revisionId })
  await expect(first).toHaveCount(1)
  await expect(manager).toBeInViewport({ ratio: 1 })
  const downloading = page.waitForEvent('download')
  await first.getByRole('button', { name: '导出 ZIP', exact: true }).click()
  const bytes = await readFile((await (await downloading).path())!)
  expect(createHash('sha256').update(bytes).digest('hex')).toBe(created.package.archiveSha256)
  await first.getByRole('button', { name: '查看文件', exact: true }).click()
  await expect(manager.getByRole('textbox', { name: '文件内容', exact: true })).toHaveValue(
    /扩展说明/,
  )
  await expect(manager.getByRole('button', { name: '保存新修订', exact: true })).toHaveCount(0)
  await manager.getByRole('button', { name: '关闭此对话框', exact: true }).click()
  await row.getByRole('button', { name: '启用', exact: true }).click()
  await expect(row).toContainText('启用')
  await row.getByRole('button', { name: '卸载', exact: true }).click()
  await page
    .getByRole('dialog', { name: '卸载扩展', exact: true })
    .getByRole('button', { name: '卸载', exact: true })
    .click()
  await expect(row).toHaveCount(0)
  await page.getByRole('button', { name: '已卸载', exact: true }).click()
  const archived = page.locator('.extensions-table').getByRole('row').filter({ hasText: name })
  await archived.getByRole('button', { name: '查看历史', exact: true }).click()
  await expect(
    manager
      .locator('.extension-revisions')
      .getByRole('row')
      .filter({ hasText: created.revisionId }),
  ).toBeVisible()
  await expect(page.locator('.el-message')).toHaveCount(0)
  await page.screenshot({
    path: '.superpowers/sdd/2026-10-24-tri-admin-greenfield/extensions-ui.png',
    fullPage: true,
    animations: 'disabled',
  })
})

test('ZIP installation and binary resource replacement use the validated original package', async ({
  page,
}) => {
  const { api, headers } = await login(page)
  await workspace(page)
  const name = 'zip-' + randomUUID().slice(0, 8),
    initial = await page.request.post(api + '/extensions', {
      headers,
      data: {
        requestId: randomUUID(),
        source: {
          kind: 'skill',
          meta: { name, description: 'ZIP fixture' },
          markdown: '# Binary asset',
          files: [
            {
              path: 'assets/icon.bin',
              mode: 420,
              contentBase64: Buffer.from([0, 255, 1]).toString('base64'),
            },
          ],
        },
      },
    })
  expect(initial.status()).toBe(201)
  const fixture = (await initial.json()).data,
    bytes = await (
      await page.request.get(
        `${api}/extensions/${fixture.id}/revisions/${fixture.revisionId}/download`,
        { headers },
      )
    ).body()
  await page.getByRole('button', { name: '安装 ZIP', exact: true }).click()
  const form = page.getByRole('dialog', { name: '安装扩展 ZIP', exact: true })
  await form.getByLabel('选择扩展 ZIP').setInputFiles({
    name: 'invalid.zip',
    mimeType: 'application/zip',
    buffer: Buffer.from('invalid'),
  })
  await form.getByRole('button', { name: '安装扩展', exact: true }).click()
  await expect(form.getByText('扩展包格式无效', { exact: true })).toBeVisible()
  await form
    .getByLabel('选择扩展 ZIP')
    .setInputFiles({ name: 'resource.zip', mimeType: 'application/zip', buffer: bytes })
  const submitted = page.waitForResponse(
    (r) => r.url() === api + '/extensions' && r.request().method() === 'POST',
  )
  await form.getByRole('button', { name: '安装扩展', exact: true }).click()
  const installed = (await (await submitted).json()).data
  await expect(form).not.toBeVisible()
  expect(installed.package.archiveSha256).toBe(fixture.package.archiveSha256)
  const row = page.locator('.extensions-table').getByRole('row').filter({ hasText: installed.id })
  await row.getByRole('button', { name: '管理文件', exact: true }).click()
  const manager = page.getByRole('dialog', { name: '扩展文件与历史', exact: true })
  await manager.getByRole('combobox', { name: '修订文件', exact: true }).click()
  await page.getByRole('option', { name: 'assets/icon.bin', exact: true }).click()
  await expect(
    manager.getByText('二进制或大文件不作文本预览，可替换资源或导出原包。', { exact: true }),
  ).toBeVisible()
  await manager.getByRole('button', { name: '替换资源', exact: true }).click()
  const file = page.getByRole('dialog', { name: '新增扩展文件', exact: true }),
    replacement = Buffer.from([0, 255, 9, 8])
  await file
    .getByLabel('选择资源文件')
    .setInputFiles({ name: 'icon.bin', mimeType: 'application/octet-stream', buffer: replacement })
  const revised = page.waitForResponse(
    (r) =>
      r.url() === `${api}/extensions/${installed.id}/revisions` && r.request().method() === 'POST',
  )
  await file.getByRole('button', { name: '保存文件修订', exact: true }).click()
  const result = (await (await revised).json()).data
  await expect(file).not.toBeVisible()
  const resource = result.package.files.find((f: { path: string }) => f.path === 'assets/icon.bin')
  expect(resource.sha256).toBe(createHash('sha256').update(replacement).digest('hex'))
  expect(resource.text).toBe(false)
})

test('stale file edits preserve the draft and global constraints keep a new immutable version', async ({
  page,
}) => {
  const { api, headers } = await login(page)
  await workspace(page)
  const created = await create(page, 'conflict-' + randomUUID().slice(0, 8)),
    row = page.locator('.extensions-table').getByRole('row').filter({ hasText: created.id })
  await row.getByRole('button', { name: '管理文件', exact: true }).click()
  const manager = page.getByRole('dialog', { name: '扩展文件与历史', exact: true }),
    editor = manager.getByRole('textbox', { name: '文件内容', exact: true })
  await expect(editor).toHaveValue(/扩展说明/)
  const original = await editor.inputValue()
  await editor.fill(original + '\nDraft held on conflict.')
  expect(
    (
      await page.request.patch(`${api}/extensions/${created.id}`, {
        headers,
        data: { version: 1, enabled: true },
      })
    ).status(),
  ).toBe(200)
  await manager.getByRole('button', { name: '保存新修订', exact: true }).click()
  await expect(manager.getByText('扩展版本已改变', { exact: true })).toBeVisible()
  await expect(editor).toHaveValue(original + '\nDraft held on conflict.')
  await expect(page.getByText('待确认扩展操作', { exact: true })).not.toBeVisible()
  await manager.getByRole('button', { name: '重新载入当前版本', exact: true }).click()
  await page
    .getByRole('dialog', { name: '重新载入扩展', exact: true })
    .getByRole('button', { name: '重新载入', exact: true })
    .click()
  await expect(editor).toHaveValue(original)
  await manager.getByRole('button', { name: '关闭此对话框', exact: true }).click()
  await page.getByRole('tab', { name: '全局约束', exact: true }).click()
  const before = (await (await page.request.get(api + '/skill-constraints', { headers })).json())
    .data
  await page
    .getByRole('textbox', { name: '全局约束内容', exact: true })
    .fill('Review changes before execution.')
  await page.getByRole('button', { name: '保存全局约束', exact: true }).click()
  await expect(
    page.getByRole('heading', { name: `全局约束 · 版本 ${before.version + 1}`, exact: true }),
  ).toBeVisible()
  const current = (await (await page.request.get(api + '/skill-constraints', { headers })).json())
    .data
  expect(current.content).toBe('Review changes before execution.')
  const history = await (
    await page.request.get(api + '/skill-constraints/revisions', { headers })
  ).json()
  expect(history.data.some((r: { version: number }) => r.version === before.version)).toBe(true)
})
test('unknown extension creation is recovered by UUID without resending or persisting content', async ({
  page,
}) => {
  const account = await login(page)
  await workspace(page)
  let writes = 0,
    request = '',
    finished = false
  await page.route('**/api/v1/extensions', async (route) => {
    if (route.request().method() !== 'POST') return route.continue()
    writes++
    request = route.request().postDataJSON().requestId
    await route.fetch()
    await route.abort()
    finished = true
  })
  await page.getByRole('button', { name: '新建 Skill', exact: true }).click()
  const form = page.getByRole('dialog', { name: '新建 Skill', exact: true })
  await form
    .getByRole('textbox', { name: '名称', exact: true })
    .fill('recover-' + randomUUID().slice(0, 8))
  await form.getByRole('textbox', { name: '描述', exact: true }).fill('PRIVATE PACKAGE DESCRIPTION')
  await form
    .getByRole('textbox', { name: 'Markdown 正文', exact: true })
    .fill('PRIVATE SCRIPT CONTENT')
  await form.getByRole('button', { name: '创建 Skill', exact: true }).click()
  await expect.poll(() => finished).toBe(true)
  await expect(page.getByText('待确认扩展操作', { exact: true })).toBeVisible()
  const stored = await page.evaluate(() =>
    Object.entries(localStorage)
      .filter(([k]) => k.startsWith('tri-admin.extensions.'))
      .map(([, v]) => v)
      .join(''),
  )
  expect(stored).toContain(request)
  expect(stored).not.toContain('PRIVATE')
  expect(writes).toBe(1)
  await page.unrouteAll({ behavior: 'wait' })
  await page.reload()
  await login(page, undefined, account)
  await workspace(page)
  await page.getByRole('button', { name: '恢复查询', exact: true }).click()
  await expect(page.getByText('待确认扩展操作', { exact: true })).not.toBeVisible()
  expect(writes).toBe(1)
})
test('readonly extension users can browse files but cannot mutate extensions or global constraints', async ({
  page,
}) => {
  const { api, headers } = await login(page, ['skills.read'])
  await workspace(page)
  await expect(page.getByRole('button', { name: '新建 Skill', exact: true })).toHaveCount(0)
  await expect(page.getByRole('button', { name: '安装 ZIP', exact: true })).toHaveCount(0)
  const name = 'readonly-' + randomUUID().slice(0, 8)
  const seeded = await page.request.post(api + '/extensions', {
    headers,
    data: {
      requestId: randomUUID(),
      source: {
        kind: 'skill',
        meta: { name, description: 'Readonly fixture' },
        markdown: '# Inspect without writing',
        files: [],
      },
    },
  })
  expect(seeded.status()).toBe(201)
  await page.getByRole('button', { name: '刷新扩展', exact: true }).click()
  await page
    .locator('.extensions-table')
    .getByRole('row')
    .filter({ hasText: name })
    .getByRole('button', { name: '管理文件', exact: true })
    .click()
  const manager = page.getByRole('dialog', { name: '扩展文件与历史', exact: true })
  await expect(manager.getByRole('textbox', { name: '文件内容', exact: true })).toHaveValue(
    /Inspect without writing/,
  )
  await expect(manager.getByRole('textbox', { name: '文件内容', exact: true })).toHaveAttribute(
    'readonly',
    '',
  )
  await expect(manager.getByRole('button', { name: '保存新修订', exact: true })).toHaveCount(0)
  await expect(manager.getByRole('button', { name: '新增文件', exact: true })).toHaveCount(0)
  await manager.getByRole('button', { name: '关闭此对话框', exact: true }).click()
  await page.getByRole('tab', { name: '全局约束', exact: true }).click()
  await expect(page.getByRole('textbox', { name: '全局约束内容', exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: '保存全局约束', exact: true })).toHaveCount(0)
})

test('failed preview integrity and a denied post-commit receipt cannot silently discard content or recovery', async ({
  page,
}) => {
  await login(page)
  await workspace(page)
  const created = await create(page, 'integrity-' + randomUUID().slice(0, 8)),
    row = page.locator('.extensions-table').getByRole('row').filter({ hasText: created.id }),
    file = created.package.files[0]
  await page.route('**/api/v1/extensions/*/revisions/*/preview?*', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        data: {
          path: file.path,
          size: file.size,
          sha256: file.sha256,
          content: 'X'.repeat(file.size),
        },
      }),
    }),
  )
  await row.getByRole('button', { name: '管理文件', exact: true }).click()
  const manager = page.getByRole('dialog', { name: '扩展文件与历史', exact: true })
  await expect(manager.getByText('扩展响应或恢复记录无效', { exact: true })).toBeVisible()
  await expect(manager.getByRole('textbox', { name: '文件内容', exact: true })).toHaveValue('')
  await expect(manager.getByRole('button', { name: '保存新修订', exact: true })).toBeDisabled()
  await manager.getByRole('button', { name: '关闭此对话框', exact: true }).click()
  await page.unrouteAll({ behavior: 'wait' })
  let writes = 0
  await page.route('**/api/v1/extensions', (route) => {
    if (route.request().method() === 'POST') writes++
    return route.continue()
  })
  await page.route('**/api/v1/extensions/*/revisions/*', (route) =>
    route.fulfill({
      status: 403,
      contentType: 'application/json',
      body: JSON.stringify({ error: { code: 'FORBIDDEN', message: 'Receipt temporarily denied' } }),
    }),
  )
  await page.getByRole('button', { name: '新建 Skill', exact: true }).click()
  const form = page.getByRole('dialog', { name: '新建 Skill', exact: true })
  await form
    .getByRole('textbox', { name: '名称', exact: true })
    .fill('receipt-' + randomUUID().slice(0, 8))
  await form.getByRole('textbox', { name: '描述', exact: true }).fill('Commit before denial')
  await form.getByRole('button', { name: '创建 Skill', exact: true }).click()
  await expect(form.getByText('Receipt temporarily denied', { exact: true })).toBeVisible()
  await expect(page.getByText('待确认扩展操作', { exact: true })).toBeVisible()
  await form.getByRole('button', { name: '取消', exact: true }).click()
  await page.unrouteAll({ behavior: 'wait' })
  await page.getByRole('button', { name: '恢复查询', exact: true }).click()
  await expect(page.getByText('待确认扩展操作', { exact: true })).not.toBeVisible()
  expect(writes).toBe(1)
})
