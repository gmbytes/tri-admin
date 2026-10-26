import { test, expect } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
test('actual generated Vue output performs typed CRUD and read only controls through its independent service', async ({
  page,
}) => {
  test.skip(
    !process.env.TRI_E2E_GENERATED_ORIGIN,
    'Set TRI_E2E_GENERATED_PROJECT to a verified representative output',
  )
  const origin = process.env.TRI_E2E_GENERATED_ORIGIN!,
    api = process.env.TRI_E2E_GENERATED_API!,
    password = await readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8'),
    sku = 'ui-' + randomUUID().slice(0, 8)
  await page.goto(origin)
  await page.getByRole('textbox', { name: '服务地址', exact: true }).fill(api)
  await page.getByRole('button', { name: '切换服务', exact: true }).click()
  async function login(username: string) {
    await page.getByRole('textbox', { name: '账号', exact: true }).fill(username)
    await page.getByRole('textbox', { name: '密码', exact: true }).fill(password)
    await page.getByRole('button', { name: '登录', exact: true }).click()
    await expect(page.getByRole('button', { name: '退出登录', exact: true })).toBeVisible()
  }
  await login('generated-admin')
  await page.getByRole('button', { name: '新增记录', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: '新增记录', exact: true })
  await dialog.getByRole('textbox', { name: '编号', exact: true }).fill(sku)
  await dialog.getByRole('spinbutton', { name: '数量', exact: true }).fill('5')
  await dialog.getByRole('spinbutton', { name: '比例', exact: true }).fill('1.25')
  await dialog.getByRole('combobox', { name: '启用', exact: true }).selectOption('true')
  await dialog
    .getByRole('textbox', { name: '发布时间', exact: true })
    .fill('2026-10-26T00:00:00.000Z')
  await dialog
    .getByRole('textbox', { name: '标签', exact: true })
    .fill(['alpha', 'beta'].join(String.fromCharCode(10)))
  await dialog.getByRole('button', { name: '保存记录', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  const row = page.getByRole('row').filter({ hasText: sku })
  await expect(row).toContainText('1.25')
  await expect(row).toContainText('alpha · beta')
  await row.getByRole('button', { name: '编辑', exact: true }).click()
  const edit = page.getByRole('dialog', { name: '编辑记录', exact: true })
  await edit.getByRole('spinbutton', { name: '数量', exact: true }).fill('6')
  await edit.getByRole('button', { name: '保存记录', exact: true }).click()
  await expect(edit).not.toBeVisible()
  await expect(row).toContainText('6')
  await page.getByRole('button', { name: '退出登录', exact: true }).click()
  await login('generated-reader')
  await expect(page.getByRole('button', { name: '新增记录', exact: true })).toHaveCount(0)
  await expect(row).toBeVisible()
  await expect(row.getByRole('button', { name: '编辑', exact: true })).toHaveCount(0)
  await expect(row.getByRole('button', { name: '删除', exact: true })).toHaveCount(0)
  await page.getByRole('button', { name: '退出登录', exact: true }).click()
  await login('generated-admin')
  page.once('dialog', (dialog) => void dialog.accept())
  await row.getByRole('button', { name: '删除', exact: true }).click()
  await expect(row).toHaveCount(0)
  await page.screenshot({
    path: '.superpowers/sdd/2026-10-24-tri-admin-greenfield/generated-ui.png',
    fullPage: true,
  })
})
