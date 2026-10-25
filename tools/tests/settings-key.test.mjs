import assert from 'node:assert/strict'
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve, sep } from 'node:path'
import test from 'node:test'
import { initializeSettingsKey } from '../init-settings-key.mjs'
async function fixture(t) {
  const base = resolve(tmpdir()),
    root = await mkdtemp(join(base, 'tri-settings-'))
  t.after(() => {
    if (!root.startsWith(base + sep)) throw Error('Unsafe fixture cleanup')
    return rm(root, { recursive: true, force: true })
  })
  return join(root, '.env')
}
test('adds independent configuration keys without replacing existing credentials or returning key material', async (t) => {
  const path = await fixture(t),
    original = 'TRI_JWT_SIGNING_KEY=existing-fixture-key\nTRI_MONGODB_URI=mongodb://fixture\n'
  await writeFile(path, original)
  const result = await initializeSettingsKey(path)
  assert.deepEqual(result, { created: true, path })
  const text = await readFile(path, 'utf8')
  assert.ok(text.startsWith(original))
  assert.equal((text.match(/^TRI_SETTINGS_KEYS=/gm) ?? []).length, 1)
  const ring = text.match(/^TRI_SETTINGS_KEYS=(.+)$/m)?.[1]
  assert.ok(/^local:[a-f0-9]{64}$/.test(ring ?? ''))
  assert.ok(!JSON.stringify(result).includes(ring))
  assert.deepEqual(await initializeSettingsKey(path), { created: false, path })
  assert.equal(await readFile(path, 'utf8'), text)
})
test('does not alter incomplete existing key settings', async (t) => {
  const path = await fixture(t),
    text = 'TRI_SETTINGS_ACTIVE_KEY=existing\n'
  await writeFile(path, text)
  await assert.rejects(initializeSettingsKey(path))
  assert.equal(await readFile(path, 'utf8'), text)
})
