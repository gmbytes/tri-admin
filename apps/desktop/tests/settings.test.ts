import { expect, it } from 'vitest'
import { profilePayload, servicePayload } from '../src/core/settings'
const original = {
  id: 'id',
  name: 'API',
  code: 'api',
  description: '',
  enabled: true,
  configuration: { kind: 'http' as const, baseUrl: 'https://example.test' },
  credentials: { configured: true, keyId: 'key', keyAvailable: true },
  version: 3,
  createdAt: '',
  updatedAt: '',
  lastCheck: null,
}
it('omits kept secrets and distinguishes explicit replacement and removal', () => {
  const draft = {
    name: 'Renamed',
    code: 'api',
    description: '',
    enabled: true,
    configuration: original.configuration,
    secretMode: 'keep' as const,
    secret: null,
  }
  expect(profilePayload(draft, original)).toMatchObject({ version: 3, name: 'Renamed' })
  expect(profilePayload(draft, original)).not.toHaveProperty('secret')
  expect(profilePayload({ ...draft, secretMode: 'clear' }, original)).toHaveProperty('secret', null)
  const secret = { kind: 'http' as const, headerName: 'X-API-Key', value: 'private' }
  expect(profilePayload({ ...draft, secretMode: 'replace', secret }, original)).toHaveProperty(
    'secret',
    secret,
  )
  expect(profilePayload({ ...draft, secret }, original)).not.toHaveProperty('secret')
})
it('requires explicit credential action for changed connections and never sends stale bindings', () => {
  expect(() =>
    profilePayload(
      {
        name: 'API',
        code: 'api',
        description: '',
        enabled: true,
        configuration: { kind: 'http', baseUrl: 'https://different.test' },
        secretMode: 'keep',
        secret: null,
      },
      original,
    ),
  ).toThrow()
  const service = {
    displayName: 'Workspace',
    loginLimitPerIpOverride: null,
    effectiveLoginLimitPerIp: 100,
    storageProfileId: 'storage',
    mailProfileId: 'mail',
    version: 4,
    updatedAt: '',
  }
  expect(servicePayload({ ...service, displayName: 'New' }, service)).toEqual({
    version: 4,
    displayName: 'New',
  })
  expect(servicePayload({ ...service, mailProfileId: null }, service)).toEqual({
    version: 4,
    mailProfileId: null,
  })
})
