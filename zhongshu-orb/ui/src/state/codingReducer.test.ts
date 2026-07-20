import { describe, expect, it } from 'vitest'
import { codingReducer, initialCodingState } from './codingReducer'
import type { OverlayToUiEvent } from '../ipc/events'

describe('codingReducer', () => {
  it('builds coding workbench state from typed events', () => {
    const events: OverlayToUiEvent[] = [
      { type: 'coding', event: { kind: 'plan_created', session_id: 's1', step_count: 2, risk: 'low' } },
      { type: 'coding', event: { kind: 'plan_step_started', session_id: 's1', step_id: '1', title: 'Inspect' } },
      { type: 'coding', event: { kind: 'worker_started', worker: 'deepseek-worker', task_id: 't1', owned_files: ['src/lib.rs'] } },
      {
        type: 'coding',
        event: {
          kind: 'patch_preview',
          path: 'src/lib.rs',
          operation: 'update',
          diff_summary: '1 file',
          diff: {
            summary: '1 removed, 1 added',
            unified_diff: '@@ -1,1 +1,1 @@\n-old\n+new',
            changed: true,
            replace_all: false,
            removed_lines: 1,
            added_lines: 1,
            before_hash: 'a',
            after_hash: 'b',
          },
        },
      },
      { type: 'coding', event: { kind: 'context_included', description: 'src/lib.rs', estimated_tokens: 128 } },
      { type: 'coding', event: { kind: 'context_pressure', pressure_percent: 72, dropped_evidence: 1, dropped_recent: 0 } },
      { type: 'coding', event: { kind: 'verification', command: 'cargo test', success: true, exit_code: 0 } },
      { type: 'coding', event: { kind: 'replay_available', conversation_id: 7, replay_execution_id: 'r1' } },
      { type: 'phase_transition', from: 'plan', to: 'verify' },
    ]

    const state = events.reduce(codingReducer, initialCodingState)

    expect(state.active).toBe(true)
    expect(state.sessionId).toBe('s1')
    expect(state.steps).toHaveLength(1)
    expect(state.workers[0]?.status).toBe('running')
    expect(state.changes[0]?.path).toBe('src/lib.rs')
    expect(state.changes[0]?.diff?.unified_diff).toContain('+new')
    expect(state.contextIncluded[0]?.estimatedTokens).toBe(128)
    expect(state.contextPressure).toBe(72)
    expect(state.verifications[0]?.success).toBe(true)
    expect(state.phase?.to).toBe('verify')
    expect(state.replay?.replayExecutionId).toBe('r1')
  })

  it('preserves patch preview detail when the patch is later applied', () => {
    const events: OverlayToUiEvent[] = [
      {
        type: 'coding',
        event: {
          kind: 'patch_preview',
          path: 'src/lib.rs',
          operation: 'update',
          diff_summary: '@@ -1 +1 @@\n-old\n+new',
        },
      },
      {
        type: 'coding',
        event: {
          kind: 'patch_applied',
          path: 'src/lib.rs',
          operation: 'update',
          changed: true,
        },
      },
    ]

    const state = events.reduce(codingReducer, initialCodingState)

    expect(state.changes[0]).toMatchObject({
      path: 'src/lib.rs',
      status: 'applied',
      changed: true,
      summary: '@@ -1 +1 @@\n-old\n+new',
    })
  })

  it('keeps an unverified worker submission distinct from failure', () => {
    const events: OverlayToUiEvent[] = [
      {
        type: 'coding',
        event: {
          kind: 'worker_started',
          worker: 'worker-a',
          task_id: 'task-a',
          owned_files: [],
        },
      },
      {
        type: 'coding',
        event: {
          kind: 'worker_completed',
          worker: 'worker-a',
          task_id: 'task-a',
          success: false,
          status: 'submitted',
        },
      },
    ]

    const state = events.reduce(codingReducer, initialCodingState)

    expect(state.workers[0]?.status).toBe('submitted')
  })

  it('resets previous run artifacts when a new delegation plan starts', () => {
    const previous = {
      ...initialCodingState,
      active: true,
      workers: [{ worker: 'old', taskId: 'old', status: 'completed' as const, ownedFiles: [] }],
      changes: [{ path: 'old.rs', operation: 'edit', summary: 'old', status: 'applied' as const }],
      verifications: [{ command: 'old test', success: true }],
      recoveryMessages: ['old recovery'],
    }

    const next = codingReducer(previous, {
      type: 'coding',
      event: { kind: 'plan_created', session_id: 'new', step_count: 2, risk: 'review-only' },
    })

    expect(next.sessionId).toBe('new')
    expect(next.workers).toEqual([])
    expect(next.changes).toEqual([])
    expect(next.verifications).toEqual([])
    expect(next.recoveryMessages).toEqual([])
  })
})
