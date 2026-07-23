import { useState } from 'react'
import { Search, Server, SlidersHorizontal, Trash2 } from 'lucide-react'
import type { MemoryEntry } from '../../ipc/events'

type MemoryPanelProps = {
  entries: MemoryEntry[]
  onToggle?: (id: string, enabled: boolean) => void
  onDelete?: (id: string) => void
}

export function MemoryPanel({ entries, onToggle, onDelete }: MemoryPanelProps) {
  const [search, setSearch] = useState('')
  const [sortBy, setSortBy] = useState<'recent' | 'confidence'>('recent')

  const filtered = search
    ? entries.filter((e) => e.content.toLowerCase().includes(search.toLowerCase()) || e.source.toLowerCase().includes(search.toLowerCase()))
    : entries

  const sorted = [...filtered].sort((a, b) => {
    if (sortBy === 'confidence') return b.confidence - a.confidence
    return b.lastUsed - a.lastUsed
  })

  if (entries.length === 0) {
    return (
      <div className="panel-empty">
        <div className="panel-empty-icon"><Server size={24} /></div>
        <strong>No memories</strong>
        <p>Memories will appear here as the agent learns from your conversations and tasks.</p>
      </div>
    )
  }

  return (
    <div className="panel-list">
      <div className="panel-list-search">
        <Search size={12} />
        <input
          type="search"
          placeholder="Search memories..."
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <button
          type="button"
          className="panel-list-sort"
          onClick={() => setSortBy((v) => (v === 'recent' ? 'confidence' : 'recent'))}
          title={`Sort by ${sortBy === 'recent' ? 'confidence' : 'recent'}`}
        >
          <SlidersHorizontal size={12} />
          <span>{sortBy === 'recent' ? 'Recent' : 'Confidence'}</span>
        </button>
      </div>
      {sorted.map((entry) => (
        <MemoryRow key={entry.id} entry={entry} onToggle={onToggle} onDelete={onDelete} />
      ))}
    </div>
  )
}

function MemoryRow({
  entry,
  onToggle,
  onDelete,
}: {
  entry: MemoryEntry
  onToggle?: (id: string, enabled: boolean) => void
  onDelete?: (id: string) => void
}) {
  const pct = Math.round(entry.confidence * 100)

  return (
    <div className="memory-row">
      <div className="memory-row-head">
        <div className="memory-row-source">
          <span className={`memory-source-tag source-${entry.source}`}>{entry.source}</span>
          <span className="memory-time">{formatTime(entry.createdAt)}</span>
        </div>
        <div className="memory-row-actions">
          {onToggle ? (
            <button
              type="button"
              className={`memory-toggle ${entry.enabled ? 'on' : 'off'}`}
              onClick={() => onToggle(entry.id, !entry.enabled)}
              title={entry.enabled ? 'Disable' : 'Enable'}
            />
          ) : null}
          {onDelete ? (
            <button type="button" className="icon-button" onClick={() => onDelete(entry.id)} title="Delete">
              <Trash2 size={12} />
            </button>
          ) : null}
        </div>
      </div>
      <p className="memory-content">{entry.content}</p>
      <div className="memory-confidence">
        <div className="memory-confidence-bar" style={{ width: `${pct}%` }} />
        <span>{pct}%</span>
      </div>
    </div>
  )
}

function formatTime(ts: number): string {
  const d = new Date(ts)
  return d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })
}
