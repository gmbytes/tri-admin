import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { inventory, normalizeRoutePath } from '../inventory.mjs'

async function fixture(t, { router = '', frontend = '', swagger = { paths: {} } } = {}) {
  const root = await mkdtemp(join(tmpdir(), 'tri-inventory-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  await mkdir(join(root, 'server/router/system'), { recursive: true })
  await mkdir(join(root, 'server/initialize'), { recursive: true })
  await mkdir(join(root, 'web/src/api'), { recursive: true })
  await mkdir(join(root, 'server/docs'), { recursive: true })
  await writeFile(join(root, 'server/router/system/users.go'), router)
  await writeFile(join(root, 'server/initialize/router.go'), 'systemRouter.InitUsers(PrivateGroup)')
  await writeFile(join(root, 'web/src/api/users.js'), frontend)
  await writeFile(join(root, 'server/docs/swagger.json'), JSON.stringify(swagger))
  return root
}

test('normalizes swagger parameters while preserving route case and wildcard semantics', () => {
  assert.equal(normalizeRoutePath('/user/{ID}/?page=2'), '/user/:ID')
  assert.equal(normalizeRoutePath('/uploads/*filepath'), '/uploads/*filepath')
  assert.equal(normalizeRoutePath('/'), '/')
  assert.throws(() => normalizeRoutePath('https://example.com/user'), /path/i)
})

test('retains runtime routes missing from Swagger and merges frontend evidence', async t => {
  const root = await fixture(t, {
    frontend: "export const list = () => service({url:'/user/list', method:'post'})",
    swagger: { paths: { '/user/list': { post: {} } } },
  })
  const routes = await inventory({ sourceRoot: root, runtimeRoutes: [
    { method: 'POST', path: '/user/list', handler: 'ListUsers' },
    { method: 'GET', path: '/user/undocumented', handler: 'Undocumented' },
  ] })
  assert.equal(routes.length, 2)
  const list = routes.find(route => route.path === '/user/list')
  assert.equal(list.owner, 'go')
  assert.equal(list.status, 'pending')
  assert.deepEqual(new Set(list.evidence.map(item => item.kind)), new Set(['runtime', 'swagger', 'frontend']))
  assert.equal(routes.find(route => route.path.endsWith('undocumented')).documented, false)
})

test('rejects duplicate runtime method and path instead of silently overwriting ownership', async t => {
  const root = await fixture(t)
  await assert.rejects(inventory({ sourceRoot: root, runtimeRoutes: [
    { method: 'POST', path: '/user/add' }, { method: 'post', path: '/user/add/' },
  ] }), /duplicate/i)
})

test('keeps methods at the same path as distinct routes', async t => {
  const root = await fixture(t)
  const routes = await inventory({ sourceRoot: root, runtimeRoutes: [
    { method: 'GET', path: '/user' }, { method: 'POST', path: '/user' },
  ] })
  assert.deepEqual(routes.map(route => route.method).sort(), ['GET', 'POST'])
})

test('discovers nested Gin groups and authorization evidence without comments creating fake routes', async t => {
  const root = await fixture(t, { router: `package system
func (s *Users) InitUsers(Router *gin.RouterGroup) {
 users := Router.Group("user").Use(middleware.OperationRecord())
 settings := users.Group("settings")
 settings.PUT("save", api.Save)
 // settings.DELETE("fake", api.Fake)
 /* settings.GET("alsoFake", api.Fake) */
}` })
  const routes = await inventory({ sourceRoot: root })
  assert.equal(routes.length, 1)
  assert.equal(routes[0].path, '/user/settings/save')
  assert.equal(routes[0].auth, 'jwt+casbin')
  assert.ok(routes[0].evidence.some(item => item.kind === 'source' && item.line === 5))
})

test('marks documentation-only endpoints separately from registered runtime routes', async t => {
  const root = await fixture(t, { swagger: { paths: { '/obsolete': { post: {} } } } })
  const routes = await inventory({ sourceRoot: root, runtimeRoutes: [{ method: 'GET', path: '/health' }] })
  assert.equal(routes.find(route => route.path === '/obsolete').status, 'unregistered')
  assert.equal(routes.find(route => route.path === '/health').registered, true)
})

test('frontend source extraction ignores stale @Router comments and supports method before url', async t => {
  const root = await fixture(t, { frontend: `// @Router /wrong [post]
export const users = () => service({method:'GET',url:'/user/current'})` })
  const routes = await inventory({ sourceRoot: root })
  assert.deepEqual(routes.map(route => route.path), ['/user/current'])
  assert.equal(routes[0].evidence[0].kind, 'frontend')
})

test('does not treat an empty runtime capture as proof that all routes are unregistered', async t => {
  const root = await fixture(t, { swagger: { paths: { '/users': { get: {} } } } })
  const routes = await inventory({ sourceRoot: root })
  assert.equal(routes[0].registered, null)
  assert.equal(routes[0].status, 'pending')
})
