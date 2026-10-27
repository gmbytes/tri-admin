import { describe, it, expect } from 'vitest'
import {
  parseAiProvider,
  parseAiRun,
  verifyAiContent,
  parseAiOperation,
  aiRecoveryKey,
  AiRecoveryStore,
  verifyAiReceipt,
} from '../src/core/ai'
const id = '111111111111111111111111',
  other = '222222222222222222222222',
  uuid = '11111111-1111-4111-8111-111111111111',
  time = '2026-10-27T00:00:00.000Z'
const provider = () => ({
  id,
  name: 'Approved model service',
  profileId: other,
  profileVersion: 1,
  bindingCurrent: true,
  models: [{ id: 'fixture-model', label: 'Fixture', maxOutputTokens: 512 }],
  enabled: true,
  version: 1,
  createdAt: time,
  updatedAt: time,
})
const run = () => ({
  id,
  ownerId: other,
  requestId: uuid,
  providerId: id,
  providerVersion: 1,
  model: 'fixture-model',
  state: 'unknown',
  version: 2,
  mayHaveBeenSubmitted: true,
  hasResponse: true,
  cancelRequested: false,
  cancelRequestId: null,
  cancelConfirmed: false,
  responseStatus: null,
  errorCode: 'UPSTREAM_INTERRUPTED',
  createdAt: time,
  updatedAt: time,
  finishedAt: null,
  expiresAt: '2026-11-03T00:00:00.000Z',
})
describe('AI models and private content', () => {
  it('accepts approved fixed profile models and rejects credentials or duplicate model identities', () => {
    expect(parseAiProvider(provider()).profileVersion).toBe(1)
    expect(() =>
      parseAiProvider({ ...provider(), headers: { authorization: 'PRIVATE-KEY' } }),
    ).toThrow()
    const duplicated = provider()
    duplicated.models.push({ ...duplicated.models[0]! })
    expect(() => parseAiProvider(duplicated)).toThrow()
    expect(() =>
      parseAiProvider({ ...provider(), models: [{ id: 'x', label: 'X', maxOutputTokens: 0 }] }),
    ).toThrow()
  })
  it('keeps unknown, cancelled and provider completion separate from successful results', () => {
    expect(parseAiRun(run()).state).toBe('unknown')
    expect(
      parseAiRun({
        ...run(),
        state: 'cancelled',
        responseStatus: 'cancelled',
        cancelRequested: true,
        cancelRequestId: uuid,
        cancelConfirmed: true,
      }),
    ).toMatchObject({ cancelConfirmed: true })
    expect(
      parseAiRun({
        ...run(),
        state: 'cancelled',
        responseStatus: 'cancelled',
        cancelConfirmed: true,
      }),
    ).toMatchObject({ cancelRequested: false, cancelConfirmed: true })
    expect(() => parseAiRun({ ...run(), state: 'ready' })).toThrow()
    expect(() => parseAiRun({ ...run(), version: Number.MAX_SAFE_INTEGER + 1 })).toThrow()
    expect(() => parseAiRun({ ...run(), cancelConfirmed: true })).toThrow()
  })
  it('verifies UTF-8 result integrity and does not expose private values in errors', async () => {
    const value = {
      input: 'PRIVATE-INPUT',
      instructions: null,
      text: 'hello',
      sha256: '2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824',
      responseStatus: 'completed',
      refused: false,
      sequence: '18446744073709551615',
    }
    expect((await verifyAiContent(value)).sequence).toBe('18446744073709551615')
    await expect(verifyAiContent({ ...value, text: 'corrupt' })).rejects.toThrow()
    try {
      await verifyAiContent({ ...value, sha256: '0'.repeat(64) })
    } catch (error) {
      expect(String(error)).not.toContain('PRIVATE-INPUT')
    }
    await expect(verifyAiContent({ ...value, sequence: 3 })).rejects.toThrow()
  })
})
describe('AI request recovery metadata', () => {
  it('keeps one original intent across reloads and verifies recovered ownership and cancellation identity', () => {
    const rows = new Map<string, string>()
    const storage = {
      getItem: (key: string) => rows.get(key) ?? null,
      setItem: (key: string, value: string) => {
        rows.set(key, value)
      },
      removeItem: (key: string) => {
        rows.delete(key)
      },
    }
    const store = new AiRecoveryStore(storage, 'http://localhost:18088/api/v1', other)
    const op = {
      requestId: uuid,
      kind: 'run.create' as const,
      runId: null,
      version: null,
      createdAt: time,
    }
    store.add(op)
    store.add(op)
    expect(new AiRecoveryStore(storage, 'http://localhost:18088/api/v1', other).read()).toEqual([
      op,
    ])
    expect(verifyAiReceipt(op, run(), other).id).toBe(id)
    expect(() => verifyAiReceipt(op, run(), id)).toThrow()
    const cancel = { ...op, kind: 'run.cancel' as const, runId: id, version: 2 }
    expect(() => verifyAiReceipt(cancel, run(), other)).toThrow()
    expect(
      verifyAiReceipt(cancel, { ...run(), cancelRequested: true, cancelRequestId: uuid }, other)
        .cancelRequested,
    ).toBe(true)
    expect(() => store.add({ ...op, input: 'PRIVATE' } as typeof op)).toThrow()
    store.remove(uuid)
    expect(store.read()).toEqual([])
  })
  it('accepts only operation IDs and versions, with no prompts or key fields', () => {
    const value = {
      requestId: uuid,
      kind: 'run.create',
      runId: null,
      version: null,
      createdAt: time,
    }
    expect(parseAiOperation(value)).toEqual(value)
    for (const extra of [
      { input: 'PRIVATE-PROMPT' },
      { apiKey: 'PRIVATE-KEY' },
      { result: 'PRIVATE-RESULT' },
    ])
      expect(() => parseAiOperation({ ...value, ...extra })).toThrow()
    expect(() =>
      parseAiOperation({ ...value, kind: 'run.cancel', runId: null, version: 1 }),
    ).toThrow()
    expect(parseAiOperation({ ...value, kind: 'run.cancel', runId: id, version: 1 }).runId).toBe(id)
  })
  it('isolates recovery by canonical server and original account', () => {
    expect(aiRecoveryKey('http://LOCALHOST:18088/api/v1/', id)).toBe(
      aiRecoveryKey('http://localhost:18088/api/v1', id),
    )
    expect(aiRecoveryKey('http://localhost:18088/api/v1', id)).not.toBe(
      aiRecoveryKey('http://localhost:18088/api/v1', other),
    )
    expect(aiRecoveryKey('http://localhost:18088/api/v1', id)).not.toBe(
      aiRecoveryKey('http://localhost:28088/api/v1', id),
    )
  })
})
