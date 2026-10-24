import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import test from 'node:test'
import { initializeDevelopmentEnv } from '../init-dev-env.mjs'

test('generates separate parseable environment entries without returning secrets', async t => {
  const root = await mkdtemp(join(tmpdir(),'tri-env-'))
  t.after(()=>rm(root,{recursive:true,force:true}))
  const path = join(root,'.env')
  const result = await initializeDevelopmentEnv(path)
  const content = await readFile(path,'utf8')
  const env = Object.fromEntries(content.trim().split('\n').map(line=>line.split(/=(.*)/s).slice(0,2)))
  assert.equal(env.TRI_ADMIN_BIND,'127.0.0.1:18088')
  assert.ok(env.TRI_JWT_SIGNING_KEY.length >= 64)
  assert.equal(new URL(env.TRI_LEGACY_DATABASE_URL).password,env.TRI_MYSQL_ROOT_PASSWORD)
  assert.equal(new URL(env.TRI_MONGODB_URI).port,'13317')
  assert.equal(env.TRI_MONGODB_DATABASE,'tri_admin_dev')
  assert.equal(new URL(env.TRI_LEGACY_DATABASE_URL).port,'13316')
  assert.deepEqual(result,{created:true,path})
  assert.ok(!JSON.stringify(result).includes(env.TRI_MYSQL_ROOT_PASSWORD))
})

test('keeps existing environment credentials intact', async t => {
  const root = await mkdtemp(join(tmpdir(),'tri-env-'))
  t.after(()=>rm(root,{recursive:true,force:true}))
  const path = join(root,'.env')
  await writeFile(path,'EXISTING=keep-me\n')
  assert.deepEqual(await initializeDevelopmentEnv(path),{created:false,path})
  assert.equal(await readFile(path,'utf8'),'EXISTING=keep-me\n')
})
