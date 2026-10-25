import { describe, expect, it } from 'vitest'
import { normalizeEndpoint } from '../src/core/endpoint'

describe('server endpoint validation', () => {
  it('canonicalizes only the new API root', () => {
    expect(normalizeEndpoint('https://EXAMPLE.com:443/')).toBe('https://example.com/api/v1')
    expect(normalizeEndpoint('http://127.0.0.1:18088/api/v1/')).toBe('http://127.0.0.1:18088/api/v1')
    expect(normalizeEndpoint('http://[::1]:18088')).toBe('http://[::1]:18088/api/v1')
  })

  it.each([
    'http://example.com', 'http://localhost.evil.test', 'ftp://example.com',
    'https://user:private@example.com', 'https://example.com?token=private',
    'https://example.com/#private', 'https://example.com/old-api', 'http://localhost:0',
  ])('rejects unsafe or ambiguous server addresses', address => {
    expect(() => normalizeEndpoint(address)).toThrow()
  })
})
