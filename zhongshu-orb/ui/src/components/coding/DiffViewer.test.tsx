import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { ChangeSetPanel } from './ChangeSetPanel'
import { DiffViewer, parseDiffLines } from './DiffViewer'
import type { ChangeState } from '../../state/codingReducer'

describe('DiffViewer', () => {
  afterEach(() => cleanup())

  it('classifies unified diff lines with stable line numbers', () => {
    const lines = parseDiffLines('@@ -10,2 +10,2 @@\n old\n-removed\n+added')

    expect(lines.map((line) => line.kind)).toEqual(['hunk', 'context', 'remove', 'add'])
    expect(lines[1]).toMatchObject({ oldLine: 10, newLine: 10 })
    expect(lines[2]).toMatchObject({ oldLine: 11 })
    expect(lines[2]).not.toHaveProperty('newLine')
    expect(lines[3]).not.toHaveProperty('oldLine')
    expect(lines[3]).toMatchObject({ newLine: 11 })
  })

  it('renders plain summaries when no diff body is available', () => {
    render(<DiffViewer path="src/lib.rs" summary="1 file changed" />)

    expect(screen.getByLabelText('Preview src/lib.rs')).toHaveTextContent('1 file changed')
    expect(screen.getByText('summary')).toBeInTheDocument()
  })
})

describe('ChangeSetPanel', () => {
  afterEach(() => cleanup())

  const changes: ChangeState[] = [
    {
      path: 'src/lib.rs',
      operation: 'update',
      status: 'applied',
      changed: true,
      summary: '@@ -1 +1 @@\n-old\n+new',
    },
    {
      path: 'README.md',
      operation: 'update',
      status: 'applied',
      changed: false,
      summary: 'unchanged',
    },
  ]

  it('filters changed files and keeps the selected preview visible', () => {
    render(<ChangeSetPanel changes={changes} />)

    expect(screen.getByLabelText('Preview src/lib.rs')).toBeInTheDocument()

    fireEvent.change(screen.getByLabelText('Filter changed files'), {
      target: { value: 'readme' },
    })

    expect(screen.getByRole('button', { name: /README\.md/ })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /src\/lib\.rs/ })).not.toBeInTheDocument()
    expect(screen.getByLabelText('Preview README.md')).toHaveTextContent('unchanged')
  })

  it('filters by change state without hiding counts', () => {
    render(<ChangeSetPanel changes={changes} />)

    fireEvent.click(within(screen.getByLabelText('Change status filter')).getByRole('button', { name: 'unchanged 1' }))

    expect(screen.getByRole('button', { name: /README\.md/ })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /src\/lib\.rs/ })).not.toBeInTheDocument()
  })
})
