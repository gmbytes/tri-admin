import assert from 'node:assert/strict'
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises'
import { resolve, join, sep } from 'node:path'
import { tmpdir } from 'node:os'
import test from 'node:test'
import { initializeOpenKey } from '../init-open-key.mjs'
test('adds a separate open key without exposing it or replacing existing keys', async (t) => {
  const base = resolve(tmpdir()),
    root = await mkdtemp(join(base, 'tri-open-key-'))
  t.after(() => {
    if (!root.startsWith(base + sep)) throw Error('Unsafe cleanup')
    return rm(root, { recursive: true, force: true })
  })
  const path = join(root, '.env'),
    original = 'TRI_JWT_SIGNING_KEY=existing-admin-fixture-key\n'
  await writeFile(path, original)
  const result = await initializeOpenKey(path),
    content = await readFile(path, 'utf8'),
    key = content.match(/^TRI_OPEN_JWT_SIGNING_KEY=(.+)$/m)?.[1]
  assert.match(key ?? '', /^[a-f0-9]{64}$/)
  assert.ok(content.startsWith(original))
  assert.deepEqual(result, { created: true, path })
  assert.ok(!JSON.stringify(result).includes(key))
  assert.deepEqual(await initializeOpenKey(path), { created: false, path })
  assert.equal(await readFile(path, 'utf8'), content)
})
test('refuses incomplete open key values without replacing them', async (t) => {
  const base = resolve(tmpdir()),
    root = await mkdtemp(join(base, 'tri-open-key-'))
  t.after(() => {
    if (!root.startsWith(base + sep)) throw Error('Unsafe cleanup')
    return rm(root, { recursive: true, force: true })
  })
  const path = join(root, '.env'),
    content = 'TRI_OPEN_JWT_SIGNING_KEY=\n'
  await writeFile(path, content)
  await assert.rejects(initializeOpenKey(path))
  assert.equal(await readFile(path, 'utf8'), content)
})
