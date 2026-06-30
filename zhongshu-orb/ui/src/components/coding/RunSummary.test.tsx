import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import type { CodingState } from '../../state/codingReducer'
import { buildRunSummary, RunSummary } from './RunSummary'

describe('RunSummary', () => {
  afterEach(() => cleanup())

  it('marks a fully applied and verified run as review ready', () => {
    const state: CodingState = {
      active: true,
      sessionId: 's1',
      risk: 'medium',
      planStepCount: 1,
      steps: [{ id: '1', title: 'Patch', status: 'done' }],
      workers: [],
      changes: [
        {
          path: 'src/lib.rs',
          operation: 'update',
          summary: 'changed',
          status: 'applied',
          changed: true,
        },
      ],
      verifications: [{ command: 'cargo test', success: true, exitCode: 0 }],
      recoveryMessages: [],
      contextIncluded: [],
      contextPressure: 44,
      replay: { replayExecutionId: 'r1' },
    }

    const summary = buildRunSummary(state)

    expect(summary[0]).toMatchObject({ label: 'Outcome', value: 'Review ready', tone: 'good' })
    expect(summary.find((item) => item.label === 'Checks')).toMatchObject({ value: '1/1 passed', tone: 'good' })

    render(<RunSummary state={state} />)

    expect(screen.getByLabelText('Run summary')).toHaveTextContent('Review ready')
    expect(screen.getByText('Available')).toBeInTheDocument()
  })

  it('raises attention when checks fail or context was dropped', () => {
    const state: CodingState = {
      active: true,
      planStepCount: 0,
      steps: [],
      workers: [{ worker: 'deepseek-worker', taskId: 'w1', status: 'conflict', ownedFiles: [], reason: 'overlap' }],
      changes: [],
      verifications: [{ command: 'cargo test', success: false, exitCode: 101 }],
      recoveryMessages: ['retry required'],
      contextIncluded: [],
      contextPressure: 92,
      droppedEvidence: 2,
      droppedRecent: 1,
    }

    const summary = buildRunSummary(state)

    expect(summary[0]).toMatchObject({ value: 'Needs attention', tone: 'bad' })
    expect(summary.find((item) => item.label === 'Context')).toMatchObject({
      value: '92%',
      tone: 'bad',
    })
  })
})
