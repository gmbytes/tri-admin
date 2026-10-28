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
  const diagnostics: string[] = []
  page.on('pageerror', (error) => diagnostics.push(error.message))
  page.on('requestfailed', (request) =>
    diagnostics.push(`${request.failure()?.errorText}: ${new URL(request.url()).pathname}`),
  )
  await page.goto('/#/login')
  await page
    .getByRole('textbox', { name: '账号', exact: true })
    .fill(username)
    .catch(async (error) => {
      const state = await page
        .evaluate(() => ({
          ready: document.readyState,
          appChildren: document.getElementById('app')?.childNodes.length ?? 0,
        }))
        .catch(() => null)
      throw new Error(
        `Login rendering failed: ${JSON.stringify({ state, diagnostics })}; ${error.message}`,
      )
    })
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
  await page.getByPlaceholder('搜索操作标识、路径或权限').fill('files.get')
  const row = page.getByRole('row').filter({ hasText: 'files.get' })
  await expect(row).toContainText('/api/v1/files/{id}')
  await expect(row).toContainText('files.read')
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
    await page.keyboard.press('Escape')
    await expect(userDialog.getByRole('combobox', { name: '角色', exact: true })).toHaveAttribute(
      'aria-expanded',
      'false',
    )
    await expect(
      userDialog.locator('.el-select__selected-item').filter({ hasText: roleName + '已修改' }),
    ).toBeVisible()
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

test('seven approved modules remain and retired role permissions and routes stay absent', async ({
  page,
}) => {
  await login(page)
  await expect(page.locator('nav').getByRole('button')).toHaveText([
    '工作台',
    '用户管理',
    '角色管理',
    '菜单管理',
    '权限目录',
    '日志中心',
    '文件管理',
  ])
  await navigate(page, '角色管理')
  await page.getByRole('button', { name: '新增角色', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: '新增角色', exact: true })
  await expect(dialog.getByRole('checkbox')).toHaveCount(14)
  await expect(dialog.getByText('查看文件', { exact: true })).toBeVisible()
  for (const label of [
    '查看字典',
    '管理参数',
    '查看配置',
    '查看生成器',
    '查看 Skills',
    '查看 MCP',
    '查看 AI 上游与运行',
    '查看工作流与会话',
  ])
    await expect(dialog.getByText(label, { exact: true })).toHaveCount(0)
  await dialog.getByRole('button', { name: '取消', exact: true }).click()
  for (const path of ['dictionaries', 'parameters', 'settings', 'mcp', 'workflows', 'codegen']) {
    await page.goto('/#/' + path)
    await expect(page).toHaveURL(/#\/dashboard$/)
  }
})
