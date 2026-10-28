import { readFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { test, expect, type Page } from '@playwright/test'
let api = '',
  username = ''
async function password() {
  return readFile(process.env.TRI_E2E_PASSWORD_FILE!, 'utf8')
}
test.beforeEach(async ({ page }, info) => {
  api = process.env.TRI_E2E_API ?? ''
  if (!api) throw Error('file UI fixture missing')
  username =
    'file-ui-' +
    createHash('sha256')
      .update(info.testId + ':' + info.retry)
      .digest('hex')
      .slice(0, 20)
  const response = await page.request.post(api + '/users', {
    headers: { authorization: `Bearer ${process.env.TRI_E2E_PROVISIONING_TOKEN}` },
    data: {
      username,
      displayName: '文件测试管理员',
      password: await password(),
      roleIds: [process.env.TRI_E2E_OWNER_ROLE],
    },
  })
  expect(response.status()).toBe(201)
  await page.addInitScript((endpoint) => localStorage.setItem('tri-admin.server', endpoint), api)
})
async function login(page: Page, name = username) {
  await page.goto('/#/login')
  await page.getByRole('textbox', { name: '账号', exact: true }).fill(name)
  await page.getByLabel('密码', { exact: true }).fill(await password())
  const reply = page.waitForResponse(
    (response) => response.url() === api + '/auth/login' && response.request().method() === 'POST',
  )
  await page.getByRole('button', { name: '登录', exact: true }).click()
  const response = await reply
  expect(response.status()).toBe(200)
  await expect(page.locator('.sidebar')).toBeVisible()
  return (await response.json()).data
}
async function openFiles(page: Page) {
  await page.locator('nav').getByRole('button', { name: '文件管理', exact: true }).click()
  await expect(page.getByRole('heading', { name: '文件管理', exact: true })).toBeVisible()
}
async function chooseCategory(page: Page, dialog: ReturnType<Page['getByRole']>, label: string) {
  const field = dialog.locator('.el-form-item').filter({ hasText: '文件分类' })
  await field.locator('.el-select__wrapper').click()
  const list = await field.getByRole('combobox').getAttribute('aria-controls')
  await page.locator(`[id="${list}"]`).getByRole('option').filter({ hasText: label }).click()
}
test('file workspace manages categories upload verified download metadata and deletion', async ({
  page,
}) => {
  test.setTimeout(60000)
  await login(page)
  await openFiles(page)
  const suffix = Date.now().toString(36),
    category = '文件分类-' + suffix,
    name = '文件验证-' + suffix + '.bin',
    bytes = Buffer.alloc(5 * 1024 * 1024 + 17, 43)
  await page.getByRole('tab', { name: '分类管理', exact: true }).click()
  await page.getByRole('button', { name: '新建分类', exact: true }).click()
  let dialog = page.getByRole('dialog', { name: '新建分类', exact: true })
  await dialog.getByLabel('分类名称', { exact: true }).fill(category)
  await dialog.getByLabel('分类标识', { exact: true }).fill('ui-file-' + suffix)
  await dialog.getByRole('button', { name: '保存分类', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  await expect(page.locator('.file-categories-table')).toContainText(category)
  await page.getByRole('tab', { name: '文件', exact: true }).click()
  await page.getByRole('button', { name: '上传文件', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '新建上传', exact: true })
  await dialog
    .getByTestId('upload-source')
    .setInputFiles({ name, mimeType: 'application/octet-stream', buffer: bytes })
  await expect(dialog.getByText('文件已校验', { exact: true })).toBeVisible()
  await chooseCategory(page, dialog, category)
  await dialog.getByRole('button', { name: '开始上传', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  await expect(
    page.locator('.file-uploads-table').getByRole('row').filter({ hasText: name }),
  ).toContainText('已发布', { timeout: 20000 })
  await page.getByRole('tab', { name: '文件', exact: true }).click()
  let row = page.locator('.files-table').getByRole('row').filter({ hasText: name })
  await expect(row).toContainText(category)
  await expect(page.locator('.file-uploads-table')).not.toBeVisible()
  await page.screenshot({
    path: '.superpowers/sdd/2026-10-24-tri-admin-greenfield/files-workspace.png',
    fullPage: true,
    animations: 'disabled',
  })
  const downloading = page.waitForEvent('download')
  await row.getByRole('button', { name: '下载', exact: true }).click()
  const download = await downloading
  expect(download.suggestedFilename()).toBe(name)
  expect(
    createHash('sha256')
      .update(await readFile((await download.path())!))
      .digest('hex'),
  ).toBe(createHash('sha256').update(bytes).digest('hex'))
  await row.getByRole('button', { name: '编辑', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '修改文件资料', exact: true })
  const renamed = 'renamed-' + suffix + '.bin'
  await dialog.getByLabel('文件名称', { exact: true }).fill(renamed)
  await dialog.getByRole('button', { name: '保存文件', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  row = page.locator('.files-table').getByRole('row').filter({ hasText: renamed })
  await expect(row).toBeVisible()
  await row.getByRole('button', { name: '删除', exact: true }).click()
  await page
    .getByRole('dialog', { name: '删除文件', exact: true })
    .getByRole('button', { name: '删除', exact: true })
    .click()
  await expect(row).not.toBeVisible()
  await page.getByRole('tab', { name: '分类管理', exact: true }).click()
  const categoryRow = page
    .locator('.file-categories-table')
    .getByRole('row')
    .filter({ hasText: category })
  await categoryRow.getByRole('button', { name: '删除', exact: true }).click()
  await page
    .getByRole('dialog', { name: '删除分类', exact: true })
    .getByRole('button', { name: '删除', exact: true })
    .click()
  await expect(categoryRow).not.toBeVisible()
})

test('file workspace restores a lost creation result and reconciles lost chunk acknowledgements', async ({
  page,
}) => {
  test.setTimeout(60000)
  let auth = await login(page)
  await openFiles(page)
  const suffix = Date.now().toString(36),
    name = 'resume-' + suffix + '.bin',
    bytes = Buffer.alloc(5 * 1024 * 1024 + 17, 61),
    creates: unknown[] = []
  let lostCreate = true,
    chunkWrites = 0,
    lostChunk = true
  await page.route('**/api/v1/uploads', async (route) => {
    if (route.request().method() !== 'POST') {
      await route.continue()
      return
    }
    creates.push(route.request().postDataJSON())
    const response = await route.fetch()
    if (lostCreate) {
      lostCreate = false
      await route.abort('failed')
    } else await route.fulfill({ response })
  })
  await page.route('**/api/v1/uploads/*/chunks/0', async (route) => {
    if (route.request().method() !== 'PUT') {
      await route.continue()
      return
    }
    chunkWrites++
    const response = await route.fetch()
    if (lostChunk) {
      lostChunk = false
      await route.abort('failed')
    } else await route.fulfill({ response })
  })
  await page.getByRole('button', { name: '上传文件', exact: true }).click()
  let dialog = page.getByRole('dialog', { name: '新建上传', exact: true })
  await dialog
    .getByTestId('upload-source')
    .setInputFiles({ name, mimeType: 'application/octet-stream', buffer: bytes })
  await expect(dialog.getByText('文件已校验', { exact: true })).toBeVisible()
  await dialog.getByRole('button', { name: '开始上传', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  let row = page.locator('.file-uploads-table').getByRole('row').filter({ hasText: name })
  await expect(row).toContainText('无法连接服务器')
  expect(creates).toHaveLength(1)
  expect(chunkWrites).toBe(0)
  await page.reload()
  auth = await login(page)
  await openFiles(page)
  await page.getByRole('tab', { name: '上传与续传', exact: true }).click()
  row = page.locator('.file-uploads-table').getByRole('row').filter({ hasText: name })
  await expect(row).toBeVisible()
  await row.getByRole('button', { name: '继续上传', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '继续上传', exact: true })
  await dialog.getByTestId('upload-source').setInputFiles({
    name: 'wrong.bin',
    mimeType: 'application/octet-stream',
    buffer: Buffer.alloc(bytes.length, 62),
  })
  await expect(dialog.getByRole('alert')).toContainText('不一致')
  expect(chunkWrites).toBe(0)
  await dialog.getByTestId('upload-source').setInputFiles({
    name: 'same-contents.bin',
    mimeType: 'application/octet-stream',
    buffer: bytes,
  })
  await expect(dialog.getByText('文件已校验', { exact: true })).toBeVisible()
  await dialog.getByRole('button', { name: '继续上传', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  await expect(row).toContainText('已发布', { timeout: 20000 })
  expect(new Set(creates.map((value) => (value as { requestId: string }).requestId)).size).toBe(1)
  expect(chunkWrites).toBe(1)
  const response = await page.request.get(api + '/uploads', {
    headers: { authorization: `Bearer ${auth.accessToken}` },
  })
  const sessions = await response.json()
  expect(sessions.meta.total).toBe(1)
  expect(sessions.data[0].name).toBe(name)
  expect(sessions.data[0].receivedCount).toBe(2)
  const records = await page.evaluate(() =>
    Object.keys(localStorage)
      .filter((key) => key.startsWith('tri-admin.uploads.v1:'))
      .map((key) => localStorage.getItem(key)!),
  )
  expect(records).toHaveLength(1)
  expect(records.join('')).not.toMatch(/accessToken|refreshToken|nativeHandle|C:\\|password/)
  await page.request.delete(`${api}/files/${sessions.data[0].fileId}?version=1`, {
    headers: { authorization: `Bearer ${auth.accessToken}` },
  })
})

test('read-only file users see their own files without upload edit delete or category controls', async ({
  page,
  browser,
}) => {
  test.setTimeout(60000)
  const owner = await login(page),
    headers = { authorization: `Bearer ${owner.accessToken}` },
    suffix = Date.now().toString(36),
    reader = 'file-reader-' + suffix
  const roleResponse = await page.request.post(api + '/roles', {
    headers,
    data: {
      name: 'file-reader-' + suffix,
      slug: 'file-reader-' + suffix,
      permissions: ['files.read', 'files.write'],
      dataScope: 'self',
      dataScopeRoleIds: [],
    },
  })
  expect(roleResponse.status()).toBe(201)
  const role = (await roleResponse.json()).data
  const userResponse = await page.request.post(api + '/users', {
    headers,
    data: {
      username: reader,
      displayName: '文件只读账号',
      password: await password(),
      roleIds: [role.id],
    },
  })
  expect(userResponse.status()).toBe(201)
  const authResponse = await page.request.post(api + '/auth/login', {
    data: { username: reader, password: await password() },
  })
  const readerAuth = (await authResponse.json()).data
  async function publish(token: string, name: string) {
    const bytes = Buffer.from('file-scope-fixture'),
      auth = { authorization: `Bearer ${token}` },
      created = await page.request.post(api + '/uploads', {
        headers: auth,
        data: {
          requestId: crypto.randomUUID(),
          name,
          mediaType: 'application/octet-stream',
          size: bytes.length,
          sha256: createHash('sha256').update(bytes).digest('hex'),
        },
      })
    expect(created.status()).toBe(201)
    const upload = (await created.json()).data
    expect(
      (
        await page.request.put(`${api}/uploads/${upload.id}/chunks/0`, {
          headers: {
            ...auth,
            'content-type': 'application/octet-stream',
            'x-chunk-sha256': createHash('sha256').update(bytes).digest('hex'),
          },
          data: bytes,
        })
      ).status(),
    ).toBe(200)
    expect(
      (
        await page.request.post(`${api}/uploads/${upload.id}/complete`, { headers: auth, data: {} })
      ).status(),
    ).toBe(202)
    await expect
      .poll(
        async () =>
          (await (await page.request.get(`${api}/uploads/${upload.id}`, { headers: auth })).json())
            .data.state,
        { timeout: 20000 },
      )
      .toBe('completed')
    return upload.id as string
  }
  const privateName = 'owner-only-' + suffix + '.bin',
    ownName = 'reader-own-' + suffix + '.bin',
    privateId = await publish(owner.accessToken, privateName),
    ownId = await publish(readerAuth.accessToken, ownName)
  expect(
    (
      await page.request.patch(`${api}/roles/${role.id}`, {
        headers,
        data: { version: 1, permissions: ['files.read'] },
      })
    ).status(),
  ).toBe(200)
  const context = await browser.newContext()
  try {
    const view = await context.newPage()
    await view.addInitScript((endpoint) => localStorage.setItem('tri-admin.server', endpoint), api)
    await login(view, reader)
    await openFiles(view)
    await expect(view.locator('.files-table')).toContainText(ownName)
    await expect(view.locator('.files-table')).not.toContainText(privateName)
    await expect(view.getByRole('button', { name: '上传文件', exact: true })).toHaveCount(0)
    await expect(
      view.locator('.files-table').getByRole('button', { name: '编辑', exact: true }),
    ).toHaveCount(0)
    await expect(
      view.locator('.files-table').getByRole('button', { name: '删除', exact: true }),
    ).toHaveCount(0)
    await expect(
      view.locator('.files-table').getByRole('button', { name: '下载', exact: true }),
    ).toHaveCount(1)
    await view.getByRole('tab', { name: '分类管理', exact: true }).click()
    await expect(view.getByRole('button', { name: '新建分类', exact: true })).toHaveCount(0)
  } finally {
    await context.close()
    await page.request.delete(`${api}/files/${privateId}?version=1`, { headers })
    await page.request.delete(`${api}/files/${ownId}?version=1`, { headers })
  }
})

test('file workspace confirms a publication after its response was lost', async ({ page }) => {
  test.setTimeout(45000)
  const auth = await login(page)
  await openFiles(page)
  const name = 'confirm-' + Date.now().toString(36) + '.bin'
  let completions = 0
  await page.route('**/api/v1/uploads/*/complete', async (route) => {
    if (route.request().method() !== 'POST') {
      await route.continue()
      return
    }
    completions++
    await route.fetch()
    await route.abort('failed')
  })
  await page.getByRole('button', { name: '上传文件', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: '新建上传', exact: true })
  await dialog.getByTestId('upload-source').setInputFiles({
    name,
    mimeType: 'application/octet-stream',
    buffer: Buffer.from('confirm publication'),
  })
  await expect(dialog.getByText('文件已校验', { exact: true })).toBeVisible()
  await dialog.getByRole('button', { name: '开始上传', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  const row = page.locator('.file-uploads-table').getByRole('row').filter({ hasText: name })
  await expect(row).toContainText('已发布', { timeout: 20000 })
  await expect(row).toContainText('上次操作待确认')
  await row.getByRole('button', { name: '查询结果', exact: true }).click()
  await expect(row).not.toContainText('上次操作待确认')
  await expect(row).not.toContainText('无法连接服务器')
  expect(completions).toBe(1)
  const response = await page.request.get(api + '/uploads', {
      headers: { authorization: `Bearer ${auth.accessToken}` },
    }),
    body = await response.json()
  await page.request.delete(`${api}/files/${body.data[0].fileId}?version=1`, {
    headers: { authorization: `Bearer ${auth.accessToken}` },
  })
})

test('file selectors search large catalogs and retain bindings outside the first page', async ({
  page,
}) => {
  test.setTimeout(120000)
  const auth = await login(page),
    headers = { authorization: `Bearer ${auth.accessToken}` },
    suffix = Date.now().toString(36),
    categories: string[] = []
  async function category(code: string, name: string) {
    const response = await page.request.post(api + '/file-categories', {
      headers,
      data: { code, name },
    })
    expect(response.status()).toBe(201)
    const row = (await response.json()).data
    categories.push(row.id)
    return row
  }
  let file: string | undefined,
    version = 1
  try {
    for (let index = 0; index < 100; index++) {
      await category(`aa-${suffix}-${String(index).padStart(3, '0')}`, '目录-' + index)
    }
    const selectedCategory = await category('zz-' + suffix, '跨页分类-' + suffix),
      selectedProfile = (
        await (
          await page.request.get(api + '/files/storage-options?search=ui-storage-119', { headers })
        ).json()
      ).data[0],
      name = 'choices-' + suffix + '.bin'
    await openFiles(page)
    await page.getByRole('button', { name: '上传文件', exact: true }).click()
    let dialog = page.getByRole('dialog', { name: '新建上传', exact: true })
    await dialog.getByTestId('upload-source').setInputFiles({
      name,
      mimeType: 'application/octet-stream',
      buffer: Buffer.from('catalog choices'),
    })
    await expect(dialog.getByText('文件已校验', { exact: true })).toBeVisible()
    async function search(fieldName: string, code: string, label: string) {
      const field = dialog.locator('.el-form-item').filter({ hasText: fieldName }),
        combo = field.getByRole('combobox')
      await field.locator('.el-select__wrapper').click()
      await combo.fill(code)
      const controls = await combo.getAttribute('aria-controls')
      await page
        .locator(`[id="${controls}"]`)
        .getByRole('option')
        .filter({ hasText: label })
        .click()
    }
    await search('文件分类', selectedCategory.code, selectedCategory.name)
    await search('文件存储', selectedProfile.code, selectedProfile.name)
    const creating = page.waitForResponse(
      (response) => response.url() === api + '/uploads' && response.request().method() === 'POST',
    )
    await dialog.getByRole('button', { name: '开始上传', exact: true }).click()
    const created = await creating
    expect(created.request().postDataJSON().storageProfileId).toBe(selectedProfile.id)
    file = (await created.json()).data.id
    await expect(dialog).not.toBeVisible()
    await expect(
      page.locator('.file-uploads-table').getByRole('row').filter({ hasText: name }),
    ).toContainText('已发布', { timeout: 20000 })
    await page.getByRole('tab', { name: '文件', exact: true }).click()
    const row = page.locator('.files-table').getByRole('row').filter({ hasText: name })
    await expect(row).toContainText(selectedCategory.name)
    await row.getByRole('button', { name: '编辑', exact: true }).click()
    dialog = page.getByRole('dialog', { name: '修改文件资料', exact: true })
    await expect(dialog.locator('.el-form-item').filter({ hasText: '文件分类' })).toContainText(
      selectedCategory.name,
    )
    await dialog.getByLabel('文件名称', { exact: true }).fill('changed-' + suffix + '.bin')
    const editing = page.waitForRequest(
      (request) => request.url() === `${api}/files/${file}` && request.method() === 'PATCH',
    )
    await dialog.getByRole('button', { name: '保存文件', exact: true }).click()
    expect((await editing).postDataJSON().categoryId).toBe(selectedCategory.id)
    await expect(dialog).not.toBeVisible()
    version = 2
    const metadata = (await (await page.request.get(`${api}/files/${file}`, { headers })).json())
      .data
    expect(metadata.categoryId).toBe(selectedCategory.id)
    expect(metadata.storageProfileId).toBe(selectedProfile.id)
  } finally {
    if (file) await page.request.delete(`${api}/files/${file}?version=${version}`, { headers })
    for (const id of categories)
      await page.request.delete(`${api}/file-categories/${id}?version=1`, { headers })
  }
})

test('upload pagination follows the latest page while an earlier refresh is pending', async ({
  page,
}) => {
  test.setTimeout(60000)
  const auth = await login(page),
    headers = { authorization: `Bearer ${auth.accessToken}` },
    suffix = Date.now().toString(36),
    ids: string[] = [],
    oldest = 'paging-' + suffix + '-00.bin'
  let pageTwoRequests = 0
  let release!: () => void,
    started!: () => void,
    hold = true
  const gate = new Promise<void>((resolve) => {
      release = resolve
    }),
    waiting = new Promise<void>((resolve) => {
      started = resolve
    })
  try {
    for (let n = 0; n < 25; n++) {
      const response = await page.request.post(api + '/uploads', {
        headers,
        data: {
          requestId: crypto.randomUUID(),
          name: `paging-${suffix}-${String(n).padStart(2, '0')}.bin`,
          mediaType: 'application/octet-stream',
          size: 0,
          sha256: createHash('sha256').update('').digest('hex'),
        },
      })
      expect(response.status()).toBe(201)
      ids.push((await response.json()).data.id)
    }
    await openFiles(page)
    await page.getByRole('tab', { name: '上传与续传', exact: true }).click()
    await expect(page.locator('.file-uploads-table')).toContainText(`paging-${suffix}-24.bin`)
    await page.route('**/api/v1/uploads?*', async (route) => {
      if (new URL(route.request().url()).searchParams.get('page') === '2') pageTwoRequests++
      const response = await route.fetch()
      if (hold && new URL(route.request().url()).searchParams.get('page') === '1') {
        hold = false
        started()
        await gate
      }
      await route.fulfill({ response }).catch(() => {})
    })
    await page.getByRole('button', { name: '刷新', exact: true }).click()
    await waiting
    await page.locator('.file-workspace .el-pager .number').filter({ hasText: /^2$/ }).click()
    await expect.poll(() => pageTwoRequests).toBeGreaterThan(0)
    release()
    await expect(page.locator('.file-uploads-table')).toContainText(oldest)
  } finally {
    release()
    for (const id of ids)
      await page.request.post(`${api}/uploads/${id}/cancel`, { headers, data: {} })
  }
})
