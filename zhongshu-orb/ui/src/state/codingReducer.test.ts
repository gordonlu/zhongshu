import { describe, expect, it } from 'vitest'
import { codingReducer, initialCodingState } from './codingReducer'
import type { OverlayToUiEvent } from '../ipc/events'

describe('codingReducer', () => {
  it('builds coding workbench state from typed events', () => {
    const events: OverlayToUiEvent[] = [
      { type: 'coding', event: { kind: 'plan_created', session_id: 's1', step_count: 2, risk: 'low' } },
      { type: 'coding', event: { kind: 'plan_step_started', session_id: 's1', step_id: '1', title: 'Inspect' } },
      { type: 'coding', event: { kind: 'worker_started', worker: 'deepseek-worker', task_id: 't1', owned_files: ['src/lib.rs'] } },
      { type: 'coding', event: { kind: 'patch_preview', path: 'src/lib.rs', operation: 'update', diff_summary: '1 file' } },
      { type: 'coding', event: { kind: 'verification', command: 'cargo test', success: true, exit_code: 0 } },
      { type: 'coding', event: { kind: 'replay_available', conversation_id: 7, replay_execution_id: 'r1' } },
    ]

    const state = events.reduce(codingReducer, initialCodingState)

    expect(state.active).toBe(true)
    expect(state.sessionId).toBe('s1')
    expect(state.steps).toHaveLength(1)
    expect(state.workers[0]?.status).toBe('running')
    expect(state.changes[0]?.path).toBe('src/lib.rs')
    expect(state.verifications[0]?.success).toBe(true)
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
})
