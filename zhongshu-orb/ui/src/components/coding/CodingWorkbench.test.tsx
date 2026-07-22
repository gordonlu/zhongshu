import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { initialCodingState, type CodingState } from '../../state/codingReducer'
import { CodingWorkbench, WORKBENCH_RENDER_LIMIT } from './CodingWorkbench'

describe('CodingWorkbench', () => {
  afterEach(() => cleanup())

  it('shows an admitted automatic routing decision separately from execution state', () => {
    render(<CodingWorkbench state={{
      ...initialCodingState,
      autoDelegation: {
        routingId: 'auto-route-1',
        strategy: 'multi_agent',
        reason: 'specialist work can proceed independently',
        workerCount: 2,
      },
    }} />)

    expect(screen.getByText('multi agent')).toBeInTheDocument()
    expect(screen.getByText('specialist work can proceed independently')).toBeInTheDocument()
    expect(screen.getByText('2 delegated employees')).toBeInTheDocument()
    expect(screen.getByText('No organization task.')).toBeInTheDocument()
  })

  it('keeps long sessions responsive by rendering the latest workbench rows', () => {
    const state: CodingState = {
      ...initialCodingState,
      active: true,
      planStepCount: 100,
      steps: Array.from({ length: 100 }, (_, index) => ({
        id: `step-${index}`,
        title: `step ${index}`,
        status: index === 99 ? 'running' : 'done',
      })),
    }

    render(<CodingWorkbench state={state} />)

    expect(screen.getByText(`${100 - WORKBENCH_RENDER_LIMIT} older items hidden`)).toBeInTheDocument()
    expect(screen.queryByText('step 19')).not.toBeInTheDocument()
    expect(screen.getByText('step 20')).toBeInTheDocument()
    expect(screen.getByText('step 99')).toBeInTheDocument()
  })

  it('shows durable dependencies and keeps abandonment behind an explicit reason', () => {
    const reconcile = vi.fn()
    const abandon = vi.fn()
    const state: CodingState = {
      ...initialCodingState,
      organizationGraphs: [{
        store_version: 4,
        graph: {
          task_id: 'mutation-1',
          nodes: [
            { id: 'claim', kind: 'claim', objective: 'Claim files', requirements: { capabilities: [], read_only: false }, state: 'succeeded' },
            { id: 'apply', kind: 'apply', objective: 'Apply reviewed patch', executor: 'worker-a', requirements: { capabilities: ['patch'], read_only: false }, state: 'recovery_required' },
          ],
          edges: [{ from: 'claim', to: 'apply', kind: 'requires' }],
          artifacts: [],
          transitions: [],
          reconciliations: [],
          effect_intents: [{
            id: 'apply:workspace:000',
            node_id: 'apply',
            expectation: { kind: 'workspace_file', path: 'src/lib.rs', before_hash: 'a', after_hash: 'b', existed_before: true },
          }],
        },
      }],
    }

    render(<CodingWorkbench state={state} onReconcileDag={reconcile} onAbandonDag={abandon} />)

    expect(screen.getByText('Requires claim (requires)')).toBeInTheDocument()
    expect(screen.getByText('1 intents')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Verify external facts' }))
    expect(reconcile).toHaveBeenCalledWith('mutation-1', 'apply')

    const abandonButton = screen.getByRole('button', { name: 'Abandon as failed' })
    expect(abandonButton).toBeDisabled()
    fireEvent.change(screen.getByPlaceholderText('Why this unknown effect is being abandoned'), {
      target: { value: 'inspected divergent file' },
    })
    fireEvent.click(abandonButton)
    expect(abandon).toHaveBeenCalledWith('mutation-1', 'apply', 'inspected divergent file')
  })
})
