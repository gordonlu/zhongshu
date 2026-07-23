import { Monitor, Globe, AlertTriangle, Network, Camera, List } from 'lucide-react'
import type { ChromeState } from '../../ipc/events'

type ChromeStatusCardProps = {
  state: ChromeState
}

export function ChromeStatusCard({ state }: ChromeStatusCardProps) {
  if (!state.connected) {
    return (
      <div className="panel-empty">
        <div className="panel-empty-icon"><Monitor size={24} /></div>
        <strong>Chrome not connected</strong>
        <p>The browser automation status will appear here once Chrome is launched by the agent.</p>
      </div>
    )
  }

  return (
    <div className="chrome-card">
      <div className="chrome-card-header">
        <div className={`chrome-status-dot ${state.busy ? 'busy' : 'idle'}`} />
        <span>{state.busy ? 'Busy' : 'Idle'}</span>
      </div>

      {state.url ? (
        <div className="chrome-card-url">
          <Globe size={12} />
          <span title={state.url}>{state.url}</span>
        </div>
      ) : null}

      <div className="chrome-card-metrics">
        <div className="chrome-metric">
          <Camera size={12} />
          <span>{state.screenshot ? 'Screenshot available' : 'No screenshot'}</span>
        </div>
        <div className="chrome-metric">
          <AlertTriangle size={12} />
          <span>{state.consoleErrors} console errors</span>
        </div>
        <div className="chrome-metric">
          <Network size={12} />
          <span>{state.networkRequests} network requests</span>
        </div>
      </div>

      {state.recentActions.length > 0 ? (
        <div className="chrome-card-actions">
          <div className="chrome-card-actions-head">
            <List size={12} />
            <span>Recent actions</span>
          </div>
          <div className="chrome-card-actions-list">
            {state.recentActions.map((action, index) => (
              <span key={index} className="chrome-action">{action}</span>
            ))}
          </div>
        </div>
      ) : null}
    </div>
  )
}
