import { useState } from 'react'
import { CheckCircle2, ChevronDown, ChevronRight, Loader2, XCircle } from 'lucide-react'
import type { ToolActivity } from '../../state/chatReducer'
import type { ToolCallEntry } from '../../ipc/events'

const iconSize = 14

export function ToolCallGroup({
  entries,
  activities,
}: {
  entries?: ToolCallEntry[]
  activities?: ToolActivity[]
}) {
  const rows = [
    ...(entries ?? []).map((entry, index) => ({
      id: `entry-${entry.name}-${index}`,
      name: entry.name,
      status: isDone(entry.status) ? 'done' as const : 'running' as const,
      success: isDone(entry.status) ? entry.status.Done.success : undefined,
    })),
    ...(activities ?? []),
  ]

  if (rows.length === 0) return null

  return (
    <div className="tool-call-group" aria-label="Tool activity">
      {rows.map((row) => (
        <ToolCallRow key={row.id} row={row} />
      ))}
    </div>
  )
}

function ToolCallRow({
  row,
}: {
  row: { id: string; name: string; status: 'running' | 'done'; success?: boolean }
}) {
  const [collapsed, setCollapsed] = useState(true)
  const running = row.status === 'running'

  return (
    <div className="tool-call-row">
      <button
        type="button"
        className="tool-call-toggle"
        onClick={() => setCollapsed((v) => !v)}
        aria-label={collapsed ? 'Expand' : 'Collapse'}
      >
        {collapsed ? <ChevronRight size={12} /> : <ChevronDown size={12} />}
      </button>
      {running ? (
        <Loader2 size={iconSize} className="spin" />
      ) : row.success ? (
        <CheckCircle2 size={iconSize} />
      ) : (
        <XCircle size={iconSize} />
      )}
      <span className="tool-call-name">{row.name}</span>
      {!collapsed ? (
        <span className="tool-call-detail">details here</span>
      ) : null}
    </div>
  )
}

function isDone(status: ToolCallEntry['status']): status is { Done: { success: boolean } } {
  return typeof status === 'object' && 'Done' in status
}
