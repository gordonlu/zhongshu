import { useState } from 'react'
import { ChevronDown, ChevronRight, FileSymlink } from 'lucide-react'
import type { CompressEntry } from '../../ipc/events'

type CompressionViewProps = {
  entries: CompressEntry[]
}

export function CompressionView({ entries }: CompressionViewProps) {
  if (entries.length === 0) {
    return (
      <div className="panel-empty">
        <div className="panel-empty-icon"><FileSymlink size={24} /></div>
        <strong>No compressions</strong>
        <p>Context compression events will appear here when long conversations are automatically summarized.</p>
      </div>
    )
  }

  return (
    <div className="panel-list">
      {entries.map((entry) => (
        <CompressRow key={entry.id} entry={entry} />
      ))}
    </div>
  )
}

function CompressRow({ entry }: { entry: CompressEntry }) {
  const [open, setOpen] = useState(false)
  const saved = entry.tokenBefore - entry.tokenAfter
  const pct = entry.tokenBefore > 0 ? Math.round((saved / entry.tokenBefore) * 100) : 0

  return (
    <div className="compress-row">
      <button type="button" className="compress-row-head" onClick={() => setOpen((v) => !v)}>
        {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        <span className="compress-row-date">{formatTime(entry.timestamp)}</span>
        <span className="compress-row-stat">{entry.messageCount} msgs</span>
        <span className="compress-row-stat">{saved.toLocaleString()} tok saved</span>
        <span className="compress-row-pct">{pct}%</span>
      </button>
      {open ? (
        <div className="compress-row-detail">
          <p><strong>Before:</strong> {entry.tokenBefore.toLocaleString()} tokens</p>
          <p><strong>After:</strong> {entry.tokenAfter.toLocaleString()} tokens</p>
          <p className="compress-summary">{entry.summary}</p>
        </div>
      ) : null}
    </div>
  )
}

function formatTime(ts: number): string {
  const d = new Date(ts)
  return d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })
}
