import { CheckCircle2, Loader2, XCircle } from 'lucide-react'
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
        <div key={row.id} className="tool-call-row">
          {row.status === 'running' ? (
            <Loader2 size={iconSize} className="spin" />
          ) : row.success ? (
            <CheckCircle2 size={iconSize} />
          ) : (
            <XCircle size={iconSize} />
          )}
          <span>{row.name}</span>
          <strong>{row.status === 'running' ? 'running' : row.success ? 'done' : 'failed'}</strong>
        </div>
      ))}
    </div>
  )
}

function isDone(status: ToolCallEntry['status']): status is { Done: { success: boolean } } {
  return typeof status === 'object' && 'Done' in status
}
