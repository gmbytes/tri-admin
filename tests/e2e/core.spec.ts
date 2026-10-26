import { readFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { test, expect, type Page, type Locator } from '@playwright/test'
let api = ''
let defaultUsername = ''
test.beforeEach(async ({ page }, testInfo) => {
  api = process.env.TRI_E2E_API ?? ''
  if (!api) throw new Error('E2E service is not provisioned')
  const token = process.env.TRI_E2E_PROVISIONING_TOKEN,
    role = process.env.TRI_E2E_OWNER_ROLE
  if (!token || !role) throw new Error('E2E provisioning identity is not available')
  defaultUsername =
    'e2e-' +
    createHash('sha256')
      .update(testInfo.testId + ':' + testInfo.retry)
      .digest('hex')
      .slice(0, 24)
  const response = await page.request.post(api + '/users', {
    headers: { authorization: `Bearer ${token}` },
    data: {
      username: defaultUsername,
      displayName: 'UI 测试管理员',
      password: await password(),
      roleIds: [role],
    },
  })
  expect(response.status()).toBe(201)
  await page.addInitScript((endpoint) => localStorage.setItem('tri-admin.server', endpoint), api)
})

async function password() {
  const path = process.env.TRI_E2E_PASSWORD_FILE
  if (!path) throw new Error('E2E credential is not provisioned')
  return readFile(path, 'utf8')
}
async function login(page: Page, username = defaultUsername, secret?: string) {
  await page.goto('/#/login')
  await page.getByRole('textbox', { name: '账号', exact: true }).fill(username)
  await page.getByLabel('密码', { exact: true }).fill(secret ?? (await password()))
  const response = page.waitForResponse(
    (response) => response.url() === api + '/auth/login' && response.request().method() === 'POST',
  )
  await page.getByRole('button', { name: '登录', exact: true }).click()
  const reply = await response
  expect(reply.status()).toBe(200)
  const body = await reply.json()
  await expect(page.locator('.sidebar')).toBeVisible()
  return body.data as { accessToken: string; refreshToken: string }
}
async function navigate(page: Page, name: string) {
  await page.locator('nav').getByRole('button', { name, exact: true }).click()
  await expect(page.getByRole('heading', { name, exact: true })).toBeVisible()
}
async function openSelect(dialog: Locator, label: string) {
  await dialog
    .locator('.el-form-item')
    .filter({ hasText: label })
    .locator('.el-select__wrapper')
    .click()
}
async function chooseSelect(page: Page, dialog: Locator, label: string, option: string) {
  const field = dialog.locator('.el-form-item').filter({ hasText: label })
  await field.locator('.el-select__wrapper').click()
  const controls = await field.getByRole('combobox').getAttribute('aria-controls')
  if (!controls) throw new Error('Select control is missing its listbox relation')
  await page
    .locator(`[id="${controls}"]`)
    .getByRole('option', { name: option, exact: true })
    .click()
}

test('login submits without reloading and opens real workspace data', async ({ page }) => {
  await login(page)
  await expect(page.getByRole('heading', { name: '工作台', exact: true })).toBeVisible()
  await expect(page.locator('.stat-value').first()).not.toHaveText('—')
  await page.locator('nav').getByRole('button', { name: '用户管理', exact: true }).click()
  await expect(page.getByRole('heading', { name: '用户管理', exact: true })).toBeVisible()
  await expect(page.locator('.member-cell').getByText('admin', { exact: true })).toBeVisible()
})

test('API catalog shows actual methods paths permissions and local filters', async ({ page }) => {
  await login(page)
  await navigate(page, '权限目录')
  await page.getByRole('tab', { name: 'API 接口', exact: true }).click()
  await expect(page.locator('.api-catalog-table')).toContainText('apiCatalog.list')
  await page.getByPlaceholder('搜索操作标识、路径或权限').fill('users.create')
  await expect(page.locator('.api-catalog-table')).toContainText('roles.assign')
  await expect(page.locator('.api-catalog-table')).toContainText('users.write')
  await page.getByPlaceholder('搜索操作标识、路径或权限').fill('parameters.get')
  const row = page.getByRole('row').filter({ hasText: 'parameters.get' })
  await expect(row).toContainText('/api/v1/parameters/{id}')
  await expect(row).toContainText('parameters.read')
  const method = page.getByRole('combobox', { name: 'HTTP 方法', exact: true })
  await page.locator('.method-select .el-select__wrapper').click()
  let controls = await method.getAttribute('aria-controls')
  await page
    .locator(`[id="${controls}"]`)
    .getByRole('option', { name: 'POST', exact: true })
    .click()
  await expect(row).toHaveCount(0)
  await page.locator('.method-select .el-select__wrapper').click()
  controls = await method.getAttribute('aria-controls')
  await page.locator(`[id="${controls}"]`).getByRole('option', { name: 'GET', exact: true }).click()
  await expect(row).toBeVisible()
  await page.getByPlaceholder('搜索操作标识、路径或权限').fill('health.')
  await expect(page.locator('.api-catalog-table')).toContainText('health.live')
  await expect(page.locator('.api-catalog-table')).toContainText('公开')
  await page.getByPlaceholder('搜索操作标识、路径或权限').fill('open.profile.get')
  await expect(page.locator('.api-catalog-table')).toContainText('应用用户身份')
  await expect(page.locator('.api-catalog-table')).toContainText('open.profile.read')
  await page.getByPlaceholder('搜索操作标识、路径或权限').fill('uploads.chunk.put')
  await page.locator('.method-select .el-select__wrapper').click()
  controls = await method.getAttribute('aria-controls')
  await page.locator(`[id="${controls}"]`).getByRole('option', { name: 'PUT', exact: true }).click()
  await expect(page.locator('.api-catalog-table')).toContainText(
    '/api/v1/uploads/{id}/chunks/{index}',
  )
  await expect(page.locator('.api-catalog-table')).toContainText('files.write')
  await page.getByRole('tab', { name: '权限操作', exact: true }).click()
  await expect(page.getByPlaceholder('搜索权限名或代码')).toBeVisible()
})

test('dictionary and item forms persist edits and guard parent deletion', async ({ page }) => {
  await login(page)
  await navigate(page, '字典管理')
  const suffix = Date.now().toString(36),
    key = `ui-dict-${suffix}`
  await page.getByRole('button', { name: '新增字典', exact: true }).click()
  let dialog = page.getByRole('dialog', { name: '新增字典', exact: true })
  await dialog.getByLabel('字典名称', { exact: true }).fill(`界面字典-${suffix}`)
  await dialog.getByLabel('字典标识', { exact: true }).fill(key)
  await dialog.getByRole('button', { name: '保存字典', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  const row = page.getByRole('row').filter({ hasText: key })
  await expect(row).toBeVisible()
  await row.getByRole('button', { name: '编辑', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '编辑字典', exact: true })
  await dialog.getByLabel('字典名称', { exact: true }).fill(`更新字典-${suffix}`)
  await dialog.getByRole('button', { name: '保存字典', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  await expect(row).toContainText(`更新字典-${suffix}`)
  await row.getByRole('button', { name: '明细', exact: true }).click()
  const drawer = page.getByRole('dialog', { name: `更新字典-${suffix} · 明细`, exact: true })
  await drawer.getByRole('button', { name: '新增明细', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '新增明细', exact: true })
  await dialog.getByLabel('明细名称', { exact: true }).fill('中国')
  await dialog.getByLabel('明细值', { exact: true }).fill('cn')
  await dialog.getByRole('button', { name: '保存明细', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  const itemRow = drawer.getByRole('row').filter({ hasText: 'cn' })
  await itemRow.getByRole('button', { name: '编辑', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '编辑明细', exact: true })
  await dialog.getByLabel('明细名称', { exact: true }).fill('中国大陆')
  await dialog.getByRole('button', { name: '保存明细', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  await expect(itemRow).toContainText('中国大陆')
  await drawer.getByRole('button', { name: '关闭此对话框', exact: true }).click()
  await row.getByRole('button', { name: '删除', exact: true }).click()
  await page
    .getByRole('dialog', { name: '删除字典', exact: true })
    .getByRole('button', { name: '删除', exact: true })
    .click()
  await expect(page.getByText('请先删除字典中的活动明细', { exact: true })).toBeVisible()
  await expect(row).toBeVisible()
  await row.getByRole('button', { name: '明细', exact: true }).click()
  await itemRow.getByRole('button', { name: '删除', exact: true }).click()
  await page
    .getByRole('dialog', { name: '删除明细', exact: true })
    .getByRole('button', { name: '删除', exact: true })
    .click()
  await expect(itemRow).toHaveCount(0)
  await drawer.getByRole('button', { name: '关闭此对话框', exact: true }).click()
  await row.getByRole('button', { name: '删除', exact: true }).click()
  await page
    .getByRole('dialog', { name: '删除字典', exact: true })
    .getByRole('button', { name: '删除', exact: true })
    .click()
  await expect(row).toHaveCount(0)
})

test('parameter form validates JSON and preserves typed values through edit and delete', async ({
  page,
}) => {
  await login(page)
  await navigate(page, '参数管理')
  const key = `ui-param-${Date.now().toString(36)}`
  await page.getByRole('button', { name: '新增参数', exact: true }).click()
  let dialog = page.getByRole('dialog', { name: '新增参数', exact: true })
  await dialog.getByLabel('参数名称', { exact: true }).fill('业务开关配置')
  await dialog.getByLabel('参数标识', { exact: true }).fill(key)
  await chooseSelect(page, dialog, '值类型', 'JSON')
  await dialog.getByLabel('参数值', { exact: true }).fill('{invalid')
  await dialog.getByRole('button', { name: '保存参数', exact: true }).click()
  await expect(dialog.getByRole('alert')).toContainText('请输入有效的 JSON')
  await dialog.getByLabel('参数值', { exact: true }).fill('{"limit":1e999}')
  await dialog.getByRole('button', { name: '保存参数', exact: true }).click()
  await expect(dialog.getByRole('alert')).toContainText('JSON 中的数字超出安全范围')
  await dialog.getByLabel('参数值', { exact: true }).fill('{"enabled":true,"limit":10}')
  let response = page.waitForResponse(
    (response) => response.url() === api + '/parameters' && response.request().method() === 'POST',
  )
  await dialog.getByRole('button', { name: '保存参数', exact: true }).click()
  let body = await (await response).json()
  expect(body.data.value).toEqual({ enabled: true, limit: 10 })
  await expect(dialog).not.toBeVisible()
  const row = page.getByRole('row').filter({ hasText: key })
  await row.getByRole('button', { name: '编辑', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '编辑参数', exact: true })
  await expect(dialog.getByLabel('参数值', { exact: true })).toHaveValue(/"enabled": true/)
  await dialog.getByLabel('参数值', { exact: true }).fill('null')
  response = page.waitForResponse(
    (response) =>
      response.url() === api + `/parameters/${body.data.id}` &&
      response.request().method() === 'PATCH',
  )
  await dialog.getByRole('button', { name: '保存参数', exact: true }).click()
  body = await (await response).json()
  expect(body.data.value).toBeNull()
  expect(body.data.version).toBe(2)
  await expect(dialog).not.toBeVisible()
  await row.getByRole('button', { name: '删除', exact: true }).click()
  await page
    .getByRole('dialog', { name: '删除参数', exact: true })
    .getByRole('button', { name: '删除', exact: true })
    .click()
  await expect(row).toHaveCount(0)
})

test('role and user forms submit real create and versioned edit requests', async ({ page }) => {
  const tokens = await login(page),
    headers = { authorization: `Bearer ${tokens.accessToken}` },
    suffix = Date.now().toString(36)
  let roleId = '',
    userId = '',
    roleVersion = 1,
    userVersion = 1
  const slug = `ui-${suffix}`,
    roleName = `UI只读-${suffix}`,
    username = `ui-${suffix}`
  try {
    await navigate(page, '角色管理')
    await page.getByRole('button', { name: '新增角色', exact: true }).click()
    const roleDialog = page.getByRole('dialog', { name: '新增角色' })
    await roleDialog.getByLabel('角色名', { exact: true }).fill(roleName)
    await roleDialog.getByLabel('角色标识', { exact: true }).fill(slug)
    await roleDialog.getByText('查看用户', { exact: true }).click()
    await expect(roleDialog.getByRole('checkbox', { name: '查看用户', exact: true })).toBeChecked()
    const roleResponse = page.waitForResponse(
      (response) => response.url() === api + '/roles' && response.request().method() === 'POST',
    )
    await roleDialog.getByRole('button', { name: '保存角色' }).click()
    const role = await (await roleResponse).json()
    roleId = role.data.id
    await expect(roleDialog).not.toBeVisible()
    const roleRow = page.getByRole('row').filter({ hasText: slug })
    await roleRow.getByRole('button', { name: '编辑', exact: true }).click()
    const editRole = page.getByRole('dialog', { name: '编辑角色' })
    await editRole.getByLabel('角色名', { exact: true }).fill(roleName + '已修改')
    await editRole.getByRole('button', { name: '保存角色' }).click()
    await expect(editRole).not.toBeVisible()
    roleVersion = 2
    await expect(roleRow).toContainText(roleName + '已修改')
    await navigate(page, '用户管理')
    await page.getByRole('button', { name: '新增用户', exact: true }).click()
    const userDialog = page.getByRole('dialog', { name: '新增用户' })
    await userDialog.getByLabel('账号', { exact: true }).fill(username)
    await userDialog.getByLabel('显示名', { exact: true }).fill('UI测试成员')
    await userDialog.getByLabel('初始密码', { exact: true }).fill('UiFixturePassword-2026')
    await openSelect(userDialog, '角色')
    await page.getByRole('option', { name: roleName + '已修改', exact: true }).click()
    const userResponse = page.waitForResponse(
      (response) => response.url() === api + '/users' && response.request().method() === 'POST',
    )
    await userDialog.getByRole('button', { name: '保存用户' }).click()
    const user = await (await userResponse).json()
    userId = user.data.id
    await expect(userDialog).not.toBeVisible()
    const row = page.getByRole('row').filter({ hasText: username })
    await row.getByRole('button', { name: '编辑', exact: true }).click()
    const editUser = page.getByRole('dialog', { name: '编辑用户' })
    await editUser.getByLabel('显示名', { exact: true }).fill('UI测试成员已修改')
    await editUser.getByRole('button', { name: '保存用户' }).click()
    await expect(editUser).not.toBeVisible()
    userVersion = 2
    await expect(row).toContainText('UI测试成员已修改')
  } finally {
    if (userId)
      await page.request.delete(`${api}/users/${userId}?version=${userVersion}`, { headers })
    if (roleId)
      await page.request.delete(`${api}/roles/${roleId}?version=${roleVersion}`, { headers })
  }
})

test('menu form creates a folder and child that opens the registered page', async ({ page }) => {
  const tokens = await login(page),
    headers = { authorization: `Bearer ${tokens.accessToken}` },
    suffix = Date.now().toString(36)
  let parentId = '',
    childId = ''
  const parentName = `UI目录-${suffix}`,
    childName = `UI子菜单-${suffix}`
  try {
    await navigate(page, '菜单管理')
    await page.getByRole('button', { name: '新增菜单', exact: true }).click()
    let dialog = page.getByRole('dialog', { name: '新增菜单' })
    await dialog.getByLabel('菜单名', { exact: true }).fill(parentName)
    await dialog.getByLabel('路径', { exact: true }).fill(`/ui-${suffix}`)
    let response = page.waitForResponse(
      (response) => response.url() === api + '/menus' && response.request().method() === 'POST',
    )
    await dialog.getByRole('button', { name: '保存菜单' }).click()
    parentId = (await (await response).json()).data.id
    await expect(dialog).not.toBeVisible()
    await page.getByRole('button', { name: '新增菜单', exact: true }).click()
    dialog = page.getByRole('dialog', { name: '新增菜单' })
    await dialog.getByLabel('菜单名', { exact: true }).fill(childName)
    await dialog.getByLabel('路径', { exact: true }).fill(`/ui-child-${suffix}`)
    await openSelect(dialog, '父级菜单')
    await page.getByRole('treeitem', { name: parentName, exact: true }).click()
    await chooseSelect(page, dialog, '页面', '用户管理')
    await chooseSelect(page, dialog, '所需权限', '查看用户')
    response = page.waitForResponse(
      (response) => response.url() === api + '/menus' && response.request().method() === 'POST',
    )
    await dialog.getByRole('button', { name: '保存菜单' }).click()
    childId = (await (await response).json()).data.id
    await expect(dialog).not.toBeVisible()
    await page.locator('nav').getByRole('button', { name: parentName, exact: true }).click()
    await page.locator('nav').getByRole('button', { name: childName, exact: true }).click()
    await expect(page.getByRole('heading', { name: '用户管理', exact: true })).toBeVisible()
    await expect(page).toHaveURL(new RegExp(`/ui-child-${suffix}$`))
  } finally {
    if (childId) await page.request.delete(`${api}/menus/${childId}?version=1`, { headers })
    if (parentId) await page.request.delete(`${api}/menus/${parentId}?version=1`, { headers })
  }
})

test('viewer sees only their account and readable role labels without role-management access', async ({
  page,
  browser,
}) => {
  const tokens = await login(page),
    headers = { authorization: `Bearer ${tokens.accessToken}` },
    suffix = Date.now().toString(36)
  const roleName = `只读成员-${suffix}`,
    username = `viewer-${suffix}`
  let roleId = '',
    userId = ''
  const context = await browser.newContext({ baseURL: 'http://localhost:1420' })
  try {
    const role = await page.request.post(api + '/roles', {
      headers,
      data: {
        name: roleName,
        slug: `viewer-${suffix}`,
        permissions: ['users.read'],
        dataScope: 'self',
      },
    })
    roleId = (await role.json()).data.id
    const user = await page.request.post(api + '/users', {
      headers,
      data: {
        username,
        displayName: '只读测试成员',
        password: 'ViewerFixturePassword-2026',
        roleIds: [roleId],
      },
    })
    userId = (await user.json()).data.id
    const viewer = await context.newPage()
    await viewer.addInitScript(
      (endpoint) => localStorage.setItem('tri-admin.server', endpoint),
      api,
    )
    await login(viewer, username, 'ViewerFixturePassword-2026')
    await expect(viewer.getByRole('heading', { name: '用户管理', exact: true })).toBeVisible()
    await expect(viewer.locator('.member-cell')).toHaveCount(1)
    await expect(viewer.locator('.table-surface')).toContainText(roleName)
    await expect(viewer.getByRole('button', { name: '新增用户', exact: true })).toHaveCount(0)
    await expect(
      viewer.locator('nav').getByRole('button', { name: '角色管理', exact: true }),
    ).toHaveCount(0)
  } finally {
    await context.close()
    if (userId) await page.request.delete(`${api}/users/${userId}?version=1`, { headers })
    if (roleId) await page.request.delete(`${api}/roles/${roleId}?version=1`, { headers })
  }
})

test('selected-role editor grants a bounded team scope that changes without a new login', async ({
  page,
  browser,
}) => {
  const tokens = await login(page),
    headers = { authorization: `Bearer ${tokens.accessToken}` },
    suffix = Date.now().toString(36)
  const teamName = `团队-${suffix}`,
    teamSlug = `zz-team-${suffix}`,
    operatorName = `scope-${suffix}`
  const userIds: string[] = [],
    teamIds: string[] = []
  let scopeId = '',
    scopeVersion = 1
  const context = await browser.newContext({ baseURL: 'http://localhost:1420' })
  try {
    for (const [name, slug] of [
      [teamName, teamSlug],
      [`外部-${suffix}`, `outside-${suffix}`],
    ]) {
      const response = await page.request.post(api + '/roles', {
        headers,
        data: { name, slug, permissions: [], dataScope: 'self', dataScopeRoleIds: [] },
      })
      expect(response.status()).toBe(201)
      teamIds.push((await response.json()).data.id)
    }
    for (const [index, roleId] of teamIds.entries()) {
      const response = await page.request.post(api + '/users', {
        headers,
        data: {
          username: `member-${index}-${suffix}`,
          displayName: index === 0 ? '范围内成员' : '范围外成员',
          password: await password(),
          roleIds: [roleId],
        },
      })
      expect(response.status()).toBe(201)
      userIds.push((await response.json()).data.id)
    }
    await navigate(page, '角色管理')
    await page.getByRole('button', { name: '新增角色', exact: true }).click()
    let dialog = page.getByRole('dialog', { name: '新增角色', exact: true })
    await dialog.getByLabel('角色名', { exact: true }).fill(`指定范围-${suffix}`)
    await dialog.getByLabel('角色标识', { exact: true }).fill(`scoped-${suffix}`)
    await dialog.getByText('指定角色', { exact: true }).click()
    await expect(dialog.getByRole('radio', { name: '指定角色', exact: true })).toBeChecked()
    const field = dialog.locator('.el-form-item').filter({ hasText: '范围角色' })
    await field.getByRole('combobox').fill(teamSlug)
    const controls = await field.getByRole('combobox').getAttribute('aria-controls')
    await page
      .locator(`[id="${controls}"]`)
      .getByRole('option', { name: `${teamName} · ${teamSlug}`, exact: true })
      .click()
    for (const label of ['查看用户', '管理用户', '分配角色'])
      await dialog.getByText(label, { exact: true }).click()
    const saved = page.waitForResponse(
      (response) => response.url() === api + '/roles' && response.request().method() === 'POST',
    )
    await dialog.getByRole('button', { name: '保存角色', exact: true }).click()
    const role = (await (await saved).json()).data
    scopeId = role.id
    expect(role.dataScopeRoleIds).toEqual([teamIds[0]])
    await expect(dialog).not.toBeVisible()
    const row = page.getByRole('row').filter({ hasText: `scoped-${suffix}` })
    await expect(row).toContainText(teamName)
    const response = await page.request.post(api + '/users', {
      headers,
      data: {
        username: operatorName,
        displayName: '范围操作员',
        password: await password(),
        roleIds: [scopeId],
      },
    })
    expect(response.status()).toBe(201)
    userIds.push((await response.json()).data.id)
    const operator = await context.newPage()
    await operator.addInitScript(
      (endpoint) => localStorage.setItem('tri-admin.server', endpoint),
      api,
    )
    await login(operator, operatorName)
    await expect(operator.locator('.member-cell')).toHaveCount(2)
    await expect(operator.locator('.table-surface')).toContainText('范围内成员')
    await expect(operator.locator('.table-surface')).not.toContainText('范围外成员')
    await expect(operator.getByRole('button', { name: '新增用户', exact: true })).toBeVisible()
    await operator.getByRole('button', { name: '新增用户', exact: true }).click()
    const memberDialog = operator.getByRole('dialog', { name: '新增用户', exact: true })
    await memberDialog.getByLabel('账号', { exact: true }).fill(`created-${suffix}`)
    await memberDialog.getByLabel('显示名', { exact: true }).fill('范围内新建成员')
    await memberDialog.getByLabel('初始密码', { exact: true }).fill(await password())
    await chooseSelect(operator, memberDialog, '角色', teamName)
    const created = operator.waitForResponse(
      (response) => response.url() === api + '/users' && response.request().method() === 'POST',
    )
    await memberDialog.getByRole('button', { name: '保存用户', exact: true }).click()
    userIds.push((await (await created).json()).data.id)
    await expect(memberDialog).not.toBeVisible()
    await expect(operator.locator('.member-cell')).toHaveCount(3)
    await row.getByRole('button', { name: '编辑', exact: true }).click()
    dialog = page.getByRole('dialog', { name: '编辑角色', exact: true })
    await dialog.getByText('自身数据', { exact: true }).click()
    await dialog.getByRole('button', { name: '保存角色', exact: true }).click()
    await expect(dialog).not.toBeVisible()
    scopeVersion = 2
    await operator.getByRole('button', { name: '刷新', exact: true }).click()
    await expect(operator.locator('.member-cell')).toHaveCount(1)
    await expect(operator.locator('.table-surface')).not.toContainText('范围内成员')
  } finally {
    await context.close()
    for (const id of userIds) await page.request.delete(`${api}/users/${id}?version=1`, { headers })
    if (scopeId)
      await page.request.delete(`${api}/roles/${scopeId}?version=${scopeVersion}`, { headers })
    for (const id of teamIds) await page.request.delete(`${api}/roles/${id}?version=1`, { headers })
  }
})

test('removed customer and client management has no navigation or API entries', async ({
  page,
}) => {
  await login(page)
  for (const name of ['客户管理', '客户端管理']) {
    await expect(page.locator('nav').getByRole('button', { name, exact: true })).toHaveCount(0)
  }
  await navigate(page, '权限目录')
  await page.getByRole('tab', { name: 'API 接口', exact: true }).click()
  for (const resource of ['customers', 'clients']) {
    await page.getByPlaceholder('搜索操作标识、路径或权限').fill(resource)
    await expect(
      page.locator('.api-catalog-table').getByRole('row').filter({ hasText: resource }),
    ).toHaveCount(0)
    await expect(page.getByText('0 个接口', { exact: true })).toBeVisible()
  }
})

test('configuration versions capture download preview conflicts and apply without overwriting stale targets', async ({
  page,
}) => {
  test.setTimeout(60000)
  const auth = await login(page)
  const headers = { authorization: `Bearer ${auth.accessToken}` }
  const suffix = Date.now().toString(36),
    key = `ui.version.${suffix}`,
    name = `版本参数-${suffix}`
  const created = await page.request.post(api + '/parameters', {
    headers,
    data: { name, key, valueType: 'boolean', value: true },
  })
  expect(created.status()).toBe(201)
  const parameter = (await created.json()).data
  await navigate(page, '版本管理')
  await page.getByRole('button', { name: '捕获配置', exact: true }).click()
  let dialog = page.getByRole('dialog', { name: '捕获配置', exact: true })
  await dialog.getByLabel('版本名称', { exact: true }).fill(`UI快照-${suffix}`)
  await dialog.getByLabel('版本编号', { exact: true }).fill(`ui-capture-${suffix}`)
  await chooseSelect(page, dialog, '参数', `${name} · ${key}`)
  await dialog.getByRole('button', { name: '保存快照', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  const row = page.getByRole('row').filter({ hasText: `ui-capture-${suffix}` })
  await expect(row).toBeVisible()
  const downloaded = page.waitForEvent('download')
  await row.getByRole('button', { name: '下载', exact: true }).click()
  const download = await downloaded
  const stream = await download.createReadStream(),
    chunks: Buffer[] = []
  if (!stream) throw Error('Missing package download')
  for await (const chunk of stream) chunks.push(Buffer.from(chunk))
  const bundle = JSON.parse(Buffer.concat(chunks).toString('utf8'))
  expect(bundle.resources.parameters[0].value).toBe(true)
  bundle.release.code = `ui-import-${suffix}`
  bundle.resources.parameters[0].value = false
  await page.getByRole('button', { name: '导入配置包', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '导入配置包', exact: true })
  await dialog.getByLabel('配置包 JSON', { exact: true }).fill(JSON.stringify(bundle))
  await dialog.getByRole('button', { name: '生成预览', exact: true }).click()
  await expect(dialog.locator('.import-summary')).toContainText('冲突')
  await expect(dialog.getByRole('button', { name: '确认应用', exact: true })).toBeDisabled()
  await dialog.getByText('合并更新', { exact: true }).click()
  await dialog.getByRole('button', { name: '生成预览', exact: true }).click()
  await expect(dialog.getByRole('button', { name: '确认应用', exact: true })).toBeEnabled()
  const edited = await page.request.patch(`${api}/parameters/${parameter.id}`, {
    headers,
    data: { version: 1, value: true },
  })
  expect(edited.status()).toBe(200)
  await dialog.getByRole('button', { name: '确认应用', exact: true }).click()
  await expect(dialog).toContainText('目标配置已改变')
  await expect(dialog.getByRole('button', { name: '确认应用', exact: true })).toBeDisabled()
  await dialog.getByRole('button', { name: '生成预览', exact: true }).click()
  await dialog.getByRole('button', { name: '确认应用', exact: true }).click()
  await expect(dialog).toContainText('配置已应用')
  expect(
    (await (await page.request.get(`${api}/parameters/${parameter.id}`, { headers })).json()).data
      .value,
  ).toBe(false)
  await dialog.getByRole('button', { name: '关闭', exact: true }).click()
  await expect(page.getByRole('row').filter({ hasText: `ui-import-${suffix}` })).toBeVisible()
})

test('version page enforces package read permissions and displays missing import writes', async ({
  page,
  browser,
}) => {
  test.setTimeout(60000)
  const owner = await login(page),
    headers = { authorization: `Bearer ${owner.accessToken}` }
  const suffix = Date.now().toString(36)
  const parameterResponse = await page.request.post(api + '/parameters', {
    headers,
    data: { name: '权限验证参数', key: `ui.access.${suffix}`, valueType: 'boolean', value: false },
  })
  expect(parameterResponse.status()).toBe(201)
  const parameter = (await parameterResponse.json()).data
  const captureResponse = await page.request.post(api + '/versions', {
    headers,
    data: { name: `权限快照-${suffix}`, code: `ui-access-${suffix}`, parameterIds: [parameter.id] },
  })
  expect(captureResponse.status()).toBe(201)
  const version = (await captureResponse.json()).data
  const packageResponse = await page.request.get(`${api}/versions/${version.id}/download`, {
    headers,
  })
  const bundle = await packageResponse.json()
  const context = await browser.newContext()
  try {
    for (const [kind, permissions] of [
      ['reader', ['versions.read']],
      ['importer', ['versions.read', 'versions.write', 'parameters.read']],
    ] as const) {
      const username = `${kind}-${suffix}`
      const roleResponse = await page.request.post(api + '/roles', {
        headers,
        data: { name: username, slug: username, permissions, dataScope: 'all' },
      })
      expect(roleResponse.status()).toBe(201)
      const role = (await roleResponse.json()).data
      const userResponse = await page.request.post(api + '/users', {
        headers,
        data: { username, displayName: username, password: await password(), roleIds: [role.id] },
      })
      expect(userResponse.status()).toBe(201)
      const operator = await context.newPage()
      await operator.addInitScript(
        (endpoint) => localStorage.setItem('tri-admin.server', endpoint),
        api,
      )
      await login(operator, username)
      await navigate(operator, '版本管理')
      if (kind === 'reader') {
        await expect(operator.getByRole('button', { name: '捕获配置', exact: true })).toHaveCount(0)
        await expect(operator.getByRole('button', { name: '导入配置包', exact: true })).toHaveCount(
          0,
        )
        const denied = operator.waitForResponse(
          (response) => response.url() === `${api}/versions/${version.id}/download`,
        )
        await operator
          .getByRole('row')
          .filter({ hasText: version.code })
          .getByRole('button', { name: '下载', exact: true })
          .click()
        expect((await denied).status()).toBe(403)
        await expect(operator.locator('.el-message--error')).toBeVisible()
      } else {
        await operator.getByRole('button', { name: '导入配置包', exact: true }).click()
        const dialog = operator.getByRole('dialog', { name: '导入配置包', exact: true })
        await dialog.getByLabel('选择配置包', { exact: true }).setInputFiles({
          name: 'invalid.json',
          mimeType: 'application/json',
          buffer: Buffer.from('{}'),
        })
        await expect(dialog).toContainText('仅支持 tri-admin/config-bundle')
        await dialog.getByLabel('选择配置包', { exact: true }).setInputFiles({
          name: 'configuration.json',
          mimeType: 'application/json',
          buffer: Buffer.from(JSON.stringify(bundle)),
        })
        await expect(dialog.getByLabel('配置包 JSON', { exact: true })).toHaveValue(
          JSON.stringify(bundle),
        )
        await dialog.getByRole('button', { name: '生成预览', exact: true }).click()
        await expect(dialog).toContainText('缺少写入权限：parameters.write')
        await expect(dialog.getByRole('button', { name: '确认应用', exact: true })).toBeDisabled()
      }
      await operator.close()
    }
  } finally {
    await context.close()
  }
})
