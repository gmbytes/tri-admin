import { describe, it, expect } from 'vitest'
import { createHash } from 'node:crypto'
import {
  parseConnection,
  parsePluginCapability,
  parseAction,
  parseCapabilities,
  parseTools,
  parseToolLines,
  parseResult,
  verifyResult,
  validateArguments,
  buildCallDocument,
  McpRecoveryStore,
  verifyMcpReceipt,
  resultLabel,
} from '../src/core/mcp'
const id = '111111111111111111111111',
  actor = '222222222222222222222222',
  run = '333333333333333333333333',
  actionId = '444444444444444444444444',
  uuid = '00000000-0000-4000-8000-000000000001',
  date = '2026-10-27T00:00:00.000Z'
const connection = {
  id,
  ownerId: actor,
  requestId: uuid,
  name: 'MCP',
  transport: { kind: 'http', profileId: run, profileVersion: 1, allowedTools: ['increment'] },
  capabilityCurrent: true,
  enabled: true,
  version: 2,
  runtime: {
    id: run,
    state: 'running',
    transportKind: 'http',
    pid: null,
    releaseConfirmed: false,
    updatedAt: date,
    errorCode: null,
  },
  createdAt: date,
  updatedAt: date,
  deletedAt: null,
}
const action = {
  id: actionId,
  actorId: actor,
  ownerId: actor,
  connectionId: id,
  requestId: uuid,
  kind: 'call',
  status: 'succeeded',
  connectionVersion: 2,
  instanceId: run,
  targetActionId: null,
  name: 'increment',
  argumentsSha256: 'a'.repeat(64),
  errorCode: null,
  resultKind: 'complete',
  resultSize: 12,
  resultSha256: 'b'.repeat(64),
  createdAt: date,
  finishedAt: date,
  cancelRequested: false,
}
describe('MCP public contracts and recovery', () => {
  it('validates immutable plugin sources and keeps their physical runtime as STDIO', () => {
    const transport = {
      kind: 'plugin',
      extensionId: id,
      revisionId: run,
      contributionId: 'records',
    }
    const plugin = {
      ...connection,
      transport,
      runtime: { ...connection.runtime, transportKind: 'stdio', pid: 42 },
    }
    expect(parseConnection(plugin).transport).toEqual(transport)
    expect(() =>
      parseConnection({ ...plugin, transport: { ...transport, program: 'unapproved' } }),
    ).toThrow()
    expect(() =>
      parseConnection({
        ...plugin,
        runtime: { ...plugin.runtime, transportKind: 'http', pid: null },
      }),
    ).toThrow()
    expect(() =>
      parseConnection({ ...plugin, transport: { ...transport, revisionId: 'old' } }),
    ).toThrow()
  })
  it('reads bounded source metadata without accepting commands or expanding an empty tool subset', () => {
    const capability = {
      extensionId: id,
      extensionName: 'record-plugin',
      revisionId: run,
      revision: 1,
      extensionVersion: 2,
      contributionId: 'records',
      label: '记录服务',
      capabilityId: 'approved-records',
      allowedTools: [],
      current: true,
    }
    expect(parsePluginCapability(capability).allowedTools).toEqual([])
    expect(() => parsePluginCapability({ ...capability, environment: {} })).toThrow()
    expect(() =>
      parsePluginCapability({ ...capability, allowedTools: ['inspect', 'inspect'] }),
    ).toThrow()
    expect(() => parsePluginCapability({ ...capability, extensionVersion: 0 })).toThrow()
    expect(() => parsePluginCapability({ ...capability, allowedTools: ['x'.repeat(65)] })).toThrow()
  })
  it('accepts the backend terminal receipts for input requests and remote tasks', () => {
    expect(
      parseAction({ ...action, status: 'inputRequired', resultKind: 'inputRequired' }).status,
    ).toBe('inputRequired')
    expect(parseAction({ ...action, status: 'remoteTask', resultKind: 'task' }).status).toBe(
      'remoteTask',
    )
    expect(parseAction({ ...action, status: 'failed' }).resultKind).toBe('complete')
  })
  it('keeps opaque tool names exactly while splitting nonblank lines', () => {
    expect(parseToolLines(' increment \r\nnew_tool\n\n')).toEqual([' increment ', 'new_tool'])
    expect(() => parseToolLines('same\nsame')).toThrow()
    expect(() => parseToolLines('   \n')).toThrow()
  })
  it('rejects private fields, fabricated HTTP PIDs and malformed current models', () => {
    expect(parseConnection(connection).runtime?.transportKind).toBe('http')
    expect(parseAction(action).kind).toBe('call')
    for (const value of [
      { ...connection, capabilityFingerprint: 'secret' },
      { ...connection, runtime: { ...connection.runtime, pid: 42 } },
      { ...connection, transport: { ...connection.transport, url: 'https://private.invalid' } },
      { ...connection, version: '2' },
      { ...connection, runtime: { ...connection.runtime, exitConfirmed: true } },
      {
        ...connection,
        transport: { ...connection.transport, allowedTools: ['increment', 'increment'] },
      },
    ])
      expect(() => parseConnection(value)).toThrow()
    expect(() => parseAction({ ...action, arguments: { password: 'secret' } })).toThrow()
    expect(() => parseAction({ ...action, status: 'success' })).toThrow()
  })
  it('bounds and validates actual tool manifests without granting their annotations', () => {
    const tools = [
      {
        name: 'increment',
        description: 'Untrusted text',
        inputSchema: { type: 'object' },
        annotations: { readOnlyHint: false },
      },
    ]
    expect(parseTools(tools)[0]?.name).toBe('increment')
    for (const value of [
      [...tools, ...tools],
      [{ ...tools[0], inputSchema: { type: 'string' } }],
      [{ ...tools[0], name: '' }],
      Array.from({ length: 129 }, (_, i) => ({ ...tools[0], name: `t${i}` })),
    ])
      expect(() => parseTools(value)).toThrow()
  })
  it('preserves approved deployment manifests with their own file budget', () => {
    const names = Array.from({ length: 64 }, (_, i) => `t${i}${'工具'.repeat(60)}`)
    expect(
      parseCapabilities([{ id: 'approved', label: 'Approved', allowedTools: names }])[0]
        ?.allowedTools,
    ).toHaveLength(64)
    expect(() =>
      parseConnection({
        ...connection,
        transport: { ...connection.transport, allowedTools: names },
      }),
    ).toThrow()
  })
  it('keeps exact integer argument text and never lets nested documents replace call identity', () => {
    const raw = '{"playerId":18446744073709551615,"nested":{"requestId":"inside"}}'
    expect(validateArguments(raw)).toBe(raw)
    const text = buildCallDocument(uuid, 2, 'increment', raw)
    expect(text).toContain('18446744073709551615')
    expect(text).toContain('"requestId":"' + uuid + '"')
    expect(text).not.toContain('18446744073709552000')
    for (const invalid of [
      'null',
      '[]',
      '{} {}',
      '{"value":1e999}',
      JSON.stringify({ data: 'x'.repeat(65536) }),
    ])
      expect(() => validateArguments(invalid)).toThrow()
    expect(() => buildCallDocument('invalid', 2, 'increment', '{}')).toThrow()
  })
  it('verifies original result bytes and treats complete tool errors and remote tasks separately', async () => {
    const content =
        '{"resultType":"complete","isError":true,"structuredContent":{"error":{"effect":"unknown"}}}',
      sha256 = createHash('sha256').update(content).digest('hex')
    const result = parseResult({
      content,
      size: Buffer.byteLength(content),
      sha256,
      kind: 'complete',
    })
    await verifyResult(result)
    expect(resultLabel(result)).toBe('远端效果未确认')
    expect(resultLabel({ ...result, kind: 'task' })).toBe('收到远端任务')
    expect(resultLabel({ ...result, kind: 'inputRequired' })).toBe('需要后续输入')
    await expect(verifyResult({ ...result, content: content + ' ' })).rejects.toThrow()
    await expect(verifyResult({ ...result, sha256: '0'.repeat(64) })).rejects.toThrow()
  })
  it('isolates recovery by server and actor and stores no parameters or credentials', () => {
    const values = new Map<string, string>(),
      storage = {
        getItem: (key: string) => values.get(key) ?? null,
        setItem: (key: string, value: string) => {
          values.set(key, value)
        },
        removeItem: (key: string) => {
          values.delete(key)
        },
      }
    const store = new McpRecoveryStore(storage, 'http://localhost:18088/api/v1', actor),
      op = {
        requestId: uuid,
        kind: 'call' as const,
        connectionId: id,
        targetActionId: null,
        version: 2,
        createdAt: date,
      }
    store.add(op)
    expect(store.read()).toEqual([op])
    expect(new McpRecoveryStore(storage, 'http://localhost:18089/api/v1', actor).read()).toEqual([])
    expect(new McpRecoveryStore(storage, 'http://localhost:18088/api/v1', id).read()).toEqual([])
    expect(() => store.add({ ...op, arguments: { token: 'secret' } } as never)).toThrow()
    expect([...values.values()].join()).not.toContain('secret')
    store.remove(uuid)
    expect(store.read()).toEqual([])
    store.add(op)
    values.set([...values.keys()][0]!, 'corrupt')
    expect(() => store.read()).toThrow()
    store.clear()
    expect(store.read()).toEqual([])
  })
  it('recovers only the original actor, connection, operation and expected configuration version', () => {
    const op = {
      requestId: uuid,
      kind: 'call' as const,
      connectionId: id,
      targetActionId: null,
      version: 2,
      createdAt: date,
    }
    expect(verifyMcpReceipt(op, action, actor).id).toBe(actionId)
    for (const value of [
      { ...action, actorId: id },
      { ...action, connectionId: run },
      { ...action, kind: 'refresh' },
      { ...action, connectionVersion: 3 },
    ])
      expect(() => verifyMcpReceipt(op, value, actor)).toThrow()
    expect(
      verifyMcpReceipt(
        { ...op, kind: 'start', version: 1 },
        { ...action, kind: 'start', name: null },
        actor,
      ),
    ).toMatchObject({ connectionVersion: 2 })
  })
})
