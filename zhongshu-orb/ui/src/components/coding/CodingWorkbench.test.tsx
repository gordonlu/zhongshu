import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import type { CodingState } from '../../state/codingReducer'
import { CodingWorkbench, WORKBENCH_RENDER_LIMIT } from './CodingWorkbench'

describe('CodingWorkbench', () => {
  afterEach(() => cleanup())

  it('keeps long sessions responsive by rendering the latest workbench rows', () => {
    const state: CodingState = {
      active: true,
      planStepCount: 100,
      steps: Array.from({ length: 100 }, (_, index) => ({
        id: `step-${index}`,
        title: `step ${index}`,
        status: index === 99 ? 'running' : 'done',
      })),
      workers: [],
      changes: [],
      verifications: [],
      recoveryMessages: [],
      contextIncluded: [],
    }

    render(<CodingWorkbench state={state} />)

    expect(screen.getByText(`${100 - WORKBENCH_RENDER_LIMIT} older items hidden`)).toBeInTheDocument()
    expect(screen.queryByText('step 19')).not.toBeInTheDocument()
    expect(screen.getByText('step 20')).toBeInTheDocument()
    expect(screen.getByText('step 99')).toBeInTheDocument()
  })
})
