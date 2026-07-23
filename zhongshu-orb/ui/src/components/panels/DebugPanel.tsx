import { useState } from 'react'
import { Bug, Code2, Clock, Activity, DollarSign, ListTree } from 'lucide-react'
import type { DebugEntry } from '../../ipc/events'

type DebugPanelProps = {
  entries: DebugEntry[]
}

const debugTabs = [
  { id: 'timeline', label: 'Timeline', icon: Clock },
  { id: 'raw', label: 'Raw Output', icon: Code2 },
  { id: 'replay', label: 'Tool Replay', icon: Activity },
  { id: 'cost', label: 'Cost / Latency', icon: DollarSign },
] as const

type DebugTabId = (typeof debugTabs)[number]['id']

export function DebugPanel({ entries }: DebugPanelProps) {
  const [tab, setTab] = useState<DebugTabId>('timeline')

  if (entries.length === 0) {
    return (
      <div className="panel-empty">
        <div className="panel-empty-icon"><Bug size={24} /></div>
        <strong>No debug data</strong>
        <p>Debug events will appear here when detailed logging is enabled.</p>
      </div>
    )
  }

  return (
    <div className="debug-panel">
      <nav className="debug-panel-tabs" aria-label="Debug tabs">
        {debugTabs.map((t) => {
          const Icon = t.icon
          return (
            <button
              key={t.id}
              type="button"
              className={`debug-tab${tab === t.id ? ' active' : ''}`}
              onClick={() => setTab(t.id)}
            >
              <Icon size={12} />
              <span>{t.label}</span>
            </button>
          )
        })}
      </nav>

      <div className="debug-panel-content">
        {tab === 'timeline' ? <TimelineView entries={entries} /> : null}
        {tab === 'raw' ? <RawOutputView entries={entries} /> : null}
        {tab === 'replay' ? <ReplayView entries={entries} /> : null}
        {tab === 'cost' ? <CostView /> : null}
      </div>
    </div>
  )
}

function TimelineView({ entries }: { entries: DebugEntry[] }) {
  return (
    <div className="debug-timeline">
      {entries.map((entry) => (
        <div key={entry.id} className={`timeline-entry type-${entry.type}`}>
          <div className="timeline-dot" />
          <div className="timeline-body">
            <div className="timeline-head">
              <span className="timeline-type">{entry.type.replace('_', ' ')}</span>
              <span className="timeline-time">{formatTime(entry.timestamp)}</span>
            </div>
            <p>{entry.summary}</p>
            {entry.details ? <pre className="timeline-details">{entry.details}</pre> : null}
          </div>
        </div>
      ))}
    </div>
  )
}

function RawOutputView({ entries }: { entries: DebugEntry[] }) {
  return (
    <div className="debug-raw">
      {entries.filter((e) => e.type === 'llm_request' || e.type === 'llm_response' || e.type === 'tool_result').map((entry) => (
        <details key={entry.id} className="debug-raw-block">
          <summary>
            <span className={`debug-raw-type type-${entry.type}`}>{entry.type.replace('_', ' ')}</span>
            <span className="debug-raw-time">{formatTime(entry.timestamp)}</span>
          </summary>
          <pre className="debug-raw-content">{entry.details || entry.summary}</pre>
        </details>
      ))}
    </div>
  )
}

function ReplayView({ entries }: { entries: DebugEntry[] }) {
  const toolEntries = entries.filter((e) => e.type === 'tool_call' || e.type === 'tool_result')

  if (toolEntries.length === 0) {
    return <p className="muted" style={{ padding: 16 }}>No tool call data to replay.</p>
  }

  return (
    <div className="debug-replay">
      {toolEntries.map((entry, index) => (
        <div key={entry.id} className={`replay-step ${entry.type === 'tool_call' ? 'call' : 'result'}`}>
          <span className="replay-index">#{index + 1}</span>
          <span className="replay-type">{entry.type === 'tool_call' ? '→' : '←'}</span>
          <span className="replay-summary">{entry.summary}</span>
          {entry.details ? <pre className="replay-details">{entry.details}</pre> : null}
        </div>
      ))}
    </div>
  )
}

function CostView() {
  return (
    <div className="debug-cost">
      <div className="panel-empty">
        <div className="panel-empty-icon"><DollarSign size={24} /></div>
        <strong>Cost data not available</strong>
        <p>Token usage and latency tracking will appear once the backend sends cost information.</p>
      </div>
    </div>
  )
}

function formatTime(ts: number): string {
  const d = new Date(ts)
  return d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit' })
}
