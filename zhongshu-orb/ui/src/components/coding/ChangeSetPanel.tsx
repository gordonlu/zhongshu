import { useMemo, useState } from 'react'
import type { ChangeState } from '../../state/codingReducer'
import { DiffViewer } from './DiffViewer'

type ChangeFilter = 'all' | 'preview' | 'applied' | 'changed' | 'unchanged'

const filters: ChangeFilter[] = ['all', 'preview', 'applied', 'changed', 'unchanged']

export function ChangeSetPanel({ changes }: { changes: ChangeState[] }) {
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<ChangeFilter>('all')
  const [selectedPath, setSelectedPath] = useState<string | null>(null)

  const filteredChanges = useMemo(() => {
    const normalizedQuery = query.trim().toLowerCase()
    return changes.filter((change) => {
      const matchesQuery = !normalizedQuery || change.path.toLowerCase().includes(normalizedQuery)
      const matchesFilter =
        filter === 'all' ||
        change.status === filter ||
        (filter === 'changed' && change.changed === true) ||
        (filter === 'unchanged' && change.changed === false)
      return matchesQuery && matchesFilter
    })
  }, [changes, filter, query])

  const selectedChange = useMemo(() => {
    if (filteredChanges.length === 0) return undefined
    return filteredChanges.find((change) => change.path === selectedPath) ?? filteredChanges[0]
  }, [filteredChanges, selectedPath])

  const counts = useMemo(() => {
    return {
      all: changes.length,
      preview: changes.filter((change) => change.status === 'preview').length,
      applied: changes.filter((change) => change.status === 'applied').length,
      changed: changes.filter((change) => change.changed === true).length,
      unchanged: changes.filter((change) => change.changed === false).length,
    }
  }, [changes])

  if (changes.length === 0) {
    return <p className="muted">No changes.</p>
  }

  return (
    <div className="change-set">
      <div className="change-toolbar">
        <input
          type="search"
          aria-label="Filter changed files"
          placeholder="Filter files"
          value={query}
          onChange={(event) => setQuery(event.currentTarget.value)}
        />
        <div className="segmented-control" aria-label="Change status filter">
          {filters.map((item) => (
            <button
              key={item}
              type="button"
              className={filter === item ? 'active' : undefined}
              onClick={() => setFilter(item)}
            >
              {item} {countForFilter(item, counts)}
            </button>
          ))}
        </div>
      </div>

      {filteredChanges.length === 0 ? <p className="muted">No files match this filter.</p> : null}

      <div className="change-list" aria-label="Changed files">
        {filteredChanges.map((change) => (
          <button
            key={change.path}
            type="button"
            className={selectedChange?.path === change.path ? 'change-item active' : 'change-item'}
            onClick={() => setSelectedPath(change.path)}
          >
            <span>{change.path}</span>
            <small>
              {change.operation} / {change.status}
              {change.changed === false ? ' / unchanged' : ''}
            </small>
          </button>
        ))}
      </div>

      {selectedChange ? (
        <DiffViewer
          path={selectedChange.path}
          summary={selectedChange.diff?.unified_diff || selectedChange.summary}
          stats={selectedChange.diff}
        />
      ) : null}
    </div>
  )
}

function countForFilter(filter: ChangeFilter, counts: Record<ChangeFilter, number>): number {
  return counts[filter]
}
