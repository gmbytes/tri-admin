import { describe, expect, it } from 'vitest'
import {
  parseWorkflowPlan,
  parseWorkflowInputs,
  parseAnalysis,
  parseWorkflowRun,
  WorkflowRecoveryStore,
  parseWorkflowOperation,
  parseWorkflowPlanRecord,
  parseWorkflowSession,
  parseWorkflowSelection,
  parseWorkflowSessionContent,
  parseWorkflowMessage,
  parseWorkflowExport,
  verifyWorkflowDefinition,
} from '../src/core/workflows'
const id = '111111111111111111111111',
  other = '222222222222222222222222'
const uuid = '11111111-1111-4111-8111-111111111111',
  time = '2026-10-27T00:00:00.000Z'
const plan = () => ({
  mode: 'feature',
  steps: [
    { id: 'analyze', title: '分析', prompt: 'PRIVATE-PROMPT', dependsOn: [], expected: 'analysis' },
    { id: 'design', title: '设计', prompt: 'Design', dependsOn: ['analyze'], expected: 'text' },
  ],
})
describe('workflow contracts', () => {
  it('validates editable DAGs without silently dropping invalid dependencies or UTF-8 prompts', () => {
    expect(parseWorkflowPlan(plan()).steps[1]?.dependsOn).toEqual(['analyze'])
    const chineseTitle = plan()
    chineseTitle.steps[0]!.title = '中'.repeat(80)
    expect(parseWorkflowPlan(chineseTitle).steps[0]?.title).toHaveLength(80)
    const cycle = plan()
    cycle.steps[0]!.dependsOn = ['design']
    expect(() => parseWorkflowPlan(cycle)).toThrow()
    const missing = plan()
    missing.steps[1]!.dependsOn = ['missing']
    expect(() => parseWorkflowPlan(missing)).toThrow()
    const duplicate = plan()
    duplicate.steps.push({ ...duplicate.steps[0]! })
    expect(() => parseWorkflowPlan(duplicate)).toThrow()
    const huge = plan()
    huge.steps[0]!.prompt = '中'.repeat(5462)
    expect(() => parseWorkflowPlan(huge)).toThrow()
    expect(() => parseWorkflowPlan({ ...plan(), execute: 'code' })).toThrow()
  })
  it('checks input byte limits and structured analysis proposals before user review', () => {
    expect(
      parseWorkflowInputs({ requirements: '需求', scene: '', constraints: '' }).requirements,
    ).toBe('需求')
    expect(() =>
      parseWorkflowInputs({ requirements: '中'.repeat(5462), scene: '', constraints: '' }),
    ).toThrow()
    const value = {
      summary: 'Summary',
      modules: [
        {
          key: 'items',
          label: '物品',
          description: '',
          fields: [{ name: 'itemId', label: '编号', kind: 'objectId', required: true }],
        },
      ],
      risks: ['风险'],
      questions: [],
    }
    expect(parseAnalysis(value).modules[0]?.fields[0]?.kind).toBe('objectId')
    expect(() => parseAnalysis({ ...value, tools: ['execute'] })).toThrow()
    value.modules[0]!.fields.push({ ...value.modules[0]!.fields[0]! })
    expect(() => parseAnalysis(value)).toThrow()
  })
  it('validates fixed plan revisions and checks their canonical digest', async () => {
    const record = {
      id,
      ownerId: other,
      requestId: uuid,
      name: '方案',
      mode: 'feature',
      stepCount: 2,
      version: 1,
      createdAt: time,
      updatedAt: time,
      deletedAt: null,
    }
    expect(parseWorkflowPlanRecord(record).stepCount).toBe(2)
    expect(() => parseWorkflowPlanRecord({ ...record, prompt: 'PRIVATE' })).toThrow()
    const canonical = JSON.stringify({
      mode: 'feature',
      steps: plan().steps.map((s) => ({
        dependsOn: s.dependsOn,
        expected: s.expected,
        id: s.id,
        prompt: s.prompt,
        title: s.title,
      })),
    })
    const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(canonical))
    const sha256 = Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, '0')).join(
      '',
    )
    const definition = { planId: id, version: 1, name: '方案', sha256, plan: plan() }
    expect((await verifyWorkflowDefinition(definition)).plan.steps).toHaveLength(2)
    await expect(
      verifyWorkflowDefinition({ ...definition, sha256: '0'.repeat(64) }),
    ).rejects.toThrow()
  })
  it('keeps session metadata separate from the encrypted draft and branch references', () => {
    const selection = {
      providerId: id,
      providerVersion: 1,
      model: 'fixture-model',
      maxOutputTokens: 512,
      stream: true,
      mode: 'feature',
      planId: null,
      planVersion: null,
    }
    expect(parseWorkflowSelection(selection).providerVersion).toBe(1)
    expect(() => parseWorkflowSelection({ ...selection, providerId: null })).toThrow()
    const metadata = {
      id,
      ownerId: other,
      requestId: uuid,
      name: '会话',
      version: 1,
      currentMessageId: null,
      messageCount: 0,
      expiresAt: time,
      createdAt: time,
      updatedAt: time,
      deletedAt: null,
    }
    expect(parseWorkflowSession(metadata).messageCount).toBe(0)
    expect(() => parseWorkflowSession({ ...metadata, draft: 'PRIVATE' })).toThrow()
    const content = {
      version: 1,
      selection,
      draft: { requirements: 'PRIVATE', scene: '', constraints: '' },
      currentMessageId: null,
      currentNode: null,
    }
    expect(parseWorkflowSessionContent(content).draft?.requirements).toBe('PRIVATE')
    expect(() => parseWorkflowSessionContent({ ...content, currentNode: '../path' })).toThrow()
    const message = {
      id,
      sessionId: other,
      requestId: uuid,
      parentMessageId: null,
      runId: id,
      sessionVersion: 2,
      createdAt: time,
    }
    expect(parseWorkflowMessage(message).parentMessageId).toBeNull()
    expect(() => parseWorkflowMessage({ ...message, runId: '../path' })).toThrow()
  })
  it('accepts only bounded Markdown manifests with the server-owned filename', () => {
    const value = {
      id,
      requestId: uuid,
      runId: other,
      runVersion: 2,
      size: 20,
      sha256: 'a'.repeat(64),
      fileName: `workflow-${other}-v2.md`,
      expiresAt: time,
      createdAt: time,
    }
    expect(parseWorkflowExport(value).size).toBe(20)
    expect(() => parseWorkflowExport({ ...value, size: 4 * 1024 * 1024 + 1 })).toThrow()
    expect(() => parseWorkflowExport({ ...value, fileName: '../../private.md' })).toThrow()
  })
  it('does not call partial, unknown or merely requested cancellation successful', () => {
    const value = {
      id,
      ownerId: other,
      requestId: uuid,
      planId: id,
      planVersion: 1,
      mode: 'feature',
      state: 'running',
      version: 2,
      completedSteps: 0,
      stepCount: 1,
      nodes: [{ id: 'analyze', state: 'unknown', aiRunId: other, errorCode: null }],
      cancelRequested: true,
      cancelConfirmed: false,
      errorCode: null,
      createdAt: time,
      updatedAt: time,
      finishedAt: null,
    }
    expect(parseWorkflowRun(value).cancelConfirmed).toBe(false)
    expect(() => parseWorkflowRun({ ...value, state: 'succeeded' })).toThrow()
    expect(() => parseWorkflowRun({ ...value, cancelConfirmed: true })).toThrow()
    expect(() => parseWorkflowRun({ ...value, completedSteps: 1 })).toThrow()
    expect(parseWorkflowRun({ ...value, state: 'cancelled', cancelConfirmed: true }).state).toBe(
      'cancelled',
    )
  })
})
describe('workflow recovery metadata', () => {
  it('persists original UUIDs without private inputs and isolates server/account', () => {
    const values = new Map<string, string>()
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => {
        values.set(key, value)
      },
      removeItem: (key: string) => {
        values.delete(key)
      },
    }
    const store = new WorkflowRecoveryStore(storage, 'http://LOCALHOST:18088/api/v1/', id)
    const op = {
      requestId: uuid,
      kind: 'run.create' as const,
      targetId: null,
      version: null,
      createdAt: time,
    }
    store.add(op)
    store.add(op)
    expect(store.read()).toEqual([op])
    expect(new WorkflowRecoveryStore(storage, 'http://localhost:18088/api/v1', id).read()).toEqual([
      op,
    ])
    expect(
      new WorkflowRecoveryStore(storage, 'http://localhost:18088/api/v1', other).read(),
    ).toEqual([])
    expect(() => parseWorkflowOperation({ ...op, inputs: 'PRIVATE' })).toThrow()
    expect(() => store.add({ ...op, kind: 'run.cancel', targetId: id, version: 1 })).toThrow()
    store.remove(uuid)
    expect(store.read()).toEqual([])
  })
})
