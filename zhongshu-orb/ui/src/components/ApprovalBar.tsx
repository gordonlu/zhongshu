import type { AuthRequest } from '../ipc/events'

export function ApprovalBar({
  request,
  onApprove,
  onDeny,
}: {
  request: AuthRequest
  onApprove: () => void
  onDeny: () => void
}) {
  return (
    <section className="approval-bar" aria-label="Approval request">
      <div className="approval-copy">
        <strong>{request.tool}</strong>
        <span>{request.source}</span>
        <span>{request.command}</span>
      </div>
      <div className="approval-actions">
        <button type="button" className="secondary-button" onClick={onDeny}>
          Deny
        </button>
        <button type="button" className="primary-button" onClick={onApprove}>
          Allow
        </button>
      </div>
    </section>
  )
}
