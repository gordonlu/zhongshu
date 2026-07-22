import { ShieldAlert, ShieldHalf, ShieldX } from 'lucide-react'
import type { AuthRequest } from '../ipc/events'

const dangerousTools = new Set(['Bash', 'Edit', 'Write', 'Create', 'Delete', 'Move', 'GitPush', 'GitCommit', 'Shell'])
const moderateTools = new Set(['Glob', 'Grep', 'Read', 'WebSearch', 'WebFetch', 'ReadFile', 'ListDir'])

function toolRisk(tool: string): 'danger' | 'moderate' | 'info' {
  if (dangerousTools.has(tool) || tool.includes('Write') || tool.includes('Delete') || tool.includes('Exec')) {
    return 'danger'
  }
  if (moderateTools.has(tool)) return 'moderate'
  return 'info'
}

function riskLabel(risk: 'danger' | 'moderate' | 'info'): string {
  switch (risk) {
    case 'danger': return 'Dangerous'
    case 'moderate': return 'Moderate'
    case 'info': return 'Info'
  }
}

function toolScope(tool: string): string {
  const writable = new Set(['Edit', 'Write', 'Create', 'Delete', 'Move', 'Rename', 'Bash', 'Shell', 'GitCommit', 'GitPush', 'GitInit', 'GitReset', 'GitCheckout'])
  const readable = new Set(['Glob', 'Grep', 'Read', 'ReadFile', 'ListDir', 'WebSearch', 'WebFetch', 'WebFetchText', 'PuppeteerEvaluate'])
  if (writable.has(tool)) return 'Writable'
  if (readable.has(tool)) return 'Read-only'
  return 'Read/write'
}

export function ApprovalBar({
  request,
  onApprove,
  onDeny,
}: {
  request: AuthRequest
  onApprove: () => void
  onDeny: () => void
}) {
  const risk = toolRisk(request.tool)

  return (
    <section className="approval-bar" aria-label="Approval request" data-risk={risk}>
      <div className="approval-header">
        {risk === 'danger' ? <ShieldX size={16} /> : risk === 'moderate' ? <ShieldHalf size={16} /> : <ShieldAlert size={16} />}
        <span className={`approval-risk approval-risk-${risk}`}>{riskLabel(risk)}</span>
        <strong className="approval-tool">{request.tool}</strong>
        <span className="approval-scope">{toolScope(request.tool)}</span>
        <span className="approval-source">{request.source}</span>
      </div>
      <div className="approval-command">
        <code>{request.command || '<no command>'}</code>
      </div>
      <div className="approval-actions">
        <button type="button" className="secondary-button" data-tooltip="Deny this request" onClick={onDeny}>
          Deny
        </button>
        <button type="button" className="primary-button" data-tooltip="Approve this request" onClick={onApprove}>
          Allow
        </button>
      </div>
    </section>
  )
}
