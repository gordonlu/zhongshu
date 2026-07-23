import { History, Search } from 'lucide-react'
import { useState } from 'react'
import type { AuthEntry } from '../../ipc/events'

type AuthHistoryPanelProps = {
  entries: AuthEntry[]
}

export function AuthHistoryPanel({ entries }: AuthHistoryPanelProps) {
  const [search, setSearch] = useState('')

  const filtered = search
    ? entries.filter((e) => e.command.toLowerCase().includes(search.toLowerCase()) || e.tool.toLowerCase().includes(search.toLowerCase()))
    : entries

  if (entries.length === 0) {
    return (
      <div className="panel-empty">
        <div className="panel-empty-icon"><History size={24} /></div>
        <strong>No auth history</strong>
        <p>Approvals and denials will appear here once you interact with tool authorization requests.</p>
      </div>
    )
  }

  return (
    <div className="panel-list">
      <div className="panel-list-search">
        <Search size={12} />
        <input
          type="search"
          placeholder="Search commands..."
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
      </div>
      {filtered.map((entry) => (
        <div key={entry.id} className={`auth-entry ${entry.approved ? 'approved' : 'denied'}`}>
          <div className="auth-entry-head">
            <span className={`auth-entry-badge ${entry.approved ? 'approve' : 'deny'}`}>
              {entry.approved ? 'Allow' : 'Deny'}
            </span>
            <strong className="auth-entry-tool">{entry.tool}</strong>
            <span className="auth-entry-time">{formatTime(entry.timestamp)}</span>
          </div>
          <code className="auth-entry-command">{entry.command}</code>
          <span className="auth-entry-source">{entry.source}</span>
        </div>
      ))}
    </div>
  )
}

function formatTime(ts: number): string {
  const d = new Date(ts)
  return d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })
}
