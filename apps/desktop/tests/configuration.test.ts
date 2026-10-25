import { expect, it } from 'vitest'
import { decodeConfigurationFile, parseConfigurationPackage } from '../src/core/configuration'
const base = { format: 'tri-admin/config-bundle', schemaVersion: 1, resources: { parameters: [] } }
it('does not silently replace invalid UTF-8 bytes when opening a configuration file', () => {
  const bytes = new TextEncoder().encode(JSON.stringify(base))
  expect(decodeConfigurationFile(bytes.buffer)).toBe(JSON.stringify(base))
  expect(() => decodeConfigurationFile(new Uint8Array([0xff]).buffer)).toThrow('UTF-8')
})
it('accepts the new package without converting its typed values', () => {
  const source = {
    ...base,
    resources: { parameters: [{ value: { enabled: false, empty: null, text: '' } }] },
  }
  expect(parseConfigurationPackage(JSON.stringify(source))).toEqual(source)
})
it('rejects malformed old or unsafe input before serializing it for preview', () => {
  for (const text of [
    '{',
    '{}',
    '{"format":"legacy","schemaVersion":1}',
    JSON.stringify(base).replace('[]', '[1e999]'),
    JSON.stringify(base).replace('[]', '[9007199254740993]'),
  ])
    expect(() => parseConfigurationPackage(text)).toThrow()
  expect(() => parseConfigurationPackage(' '.repeat(8 * 1024 * 1024 + 1))).toThrow('8 MiB')
})
