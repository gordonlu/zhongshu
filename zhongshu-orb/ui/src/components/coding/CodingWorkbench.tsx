import type { CodingState } from '../../state/codingReducer'
import { ChangeSetPanel } from './ChangeSetPanel'

export function CodingWorkbench({ state }: { state: CodingState }) {
  const failedChecks = state.verifications.filter((verification) => !verification.success).length
  const activeWorkers = state.workers.filter((worker) => worker.status === 'running').length

  return (
    <aside className="workbench" aria-label="Coding workbench">
      <header className="workbench-header">
        <div>
          <span>Agent run</span>
          <strong>{state.active ? 'In progress' : 'Standby'}</strong>
        </div>
        <div className="workbench-metrics" aria-label="Run metrics">
          <span>{state.steps.length} steps</span>
          <span>{activeWorkers} active</span>
          <span>{failedChecks} failed</span>
        </div>
      </header>

      <section className="workbench-section">
        <div className="section-heading">
          <h2>Plan</h2>
          <span>{state.steps.length}</span>
        </div>
        {state.steps.length === 0 ? <p className="muted">Waiting for plan events.</p> : null}
        {state.steps.map((step) => (
          <div key={step.id} className="workbench-row">
            <span>{step.title}</span>
            <strong>{step.status}</strong>
          </div>
        ))}
      </section>

      <section className="workbench-section">
        <div className="section-heading">
          <h2>Agents</h2>
          <span>{state.workers.length}</span>
        </div>
        {state.workers.length === 0 ? <p className="muted">No delegated agent activity.</p> : null}
        {state.workers.map((worker) => (
          <div key={worker.taskId} className={`workbench-row ${worker.status}`}>
            <span title={worker.reason ?? worker.ownedFiles.join(', ')}>
              {worker.worker}
              {worker.reason ? ` - ${worker.reason}` : ''}
            </span>
            <strong>{worker.status}</strong>
          </div>
        ))}
      </section>

      <section className="workbench-section">
        <div className="section-heading">
          <h2>Context</h2>
          {state.contextPressure !== undefined ? <span>{state.contextPressure}%</span> : null}
        </div>
        {state.contextPressure === undefined && state.contextIncluded.length === 0 ? <p className="muted">No context events.</p> : null}
        {state.contextPressure !== undefined ? (
          <div className="context-meter" aria-label="Context pressure">
            <span style={{ width: `${state.contextPressure}%` }} />
            <strong>{state.contextPressure}%</strong>
          </div>
        ) : null}
        {state.droppedEvidence || state.droppedRecent ? (
          <p className="muted">Dropped evidence {state.droppedEvidence ?? 0}, recent {state.droppedRecent ?? 0}</p>
        ) : null}
        {state.contextIncluded.map((item, index) => (
          <div key={`${item.description}-${index}`} className="workbench-row">
            <span>{item.description}</span>
            <strong>{item.estimatedTokens}</strong>
          </div>
        ))}
      </section>

      <section className="workbench-section">
        <div className="section-heading">
          <h2>Review</h2>
          <span>{state.changes.length}</span>
        </div>
        <ChangeSetPanel changes={state.changes} />
      </section>

      <section className="workbench-section">
        <div className="section-heading">
          <h2>Verification</h2>
          <span>{state.verifications.length}</span>
        </div>
        {state.verifications.length === 0 ? <p className="muted">No checks.</p> : null}
        {state.verifications.map((verification, index) => (
          <div key={`${verification.command}-${index}`} className={verification.success ? 'check-pass' : 'check-fail'}>
            <span>{verification.success ? 'pass' : 'fail'}</span>
            <code>{verification.command}</code>
          </div>
        ))}
      </section>

      <section className="workbench-section">
        <div className="section-heading">
          <h2>Recovery</h2>
          <span>{state.recoveryMessages.length}</span>
        </div>
        {state.recoveryMessages.length === 0 ? <p className="muted">No recovery feedback.</p> : null}
        {state.recoveryMessages.map((message, index) => (
          <div key={`${message}-${index}`} className="workbench-row conflict">
            <span>{message}</span>
            <strong>attention</strong>
          </div>
        ))}
      </section>

      {state.replay ? (
        <section className="workbench-section">
          <div className="section-heading">
            <h2>Replay</h2>
          </div>
          <p className="muted">
            {state.replay.replayExecutionId ?? state.replay.conversationId ?? 'available'}
          </p>
        </section>
      ) : null}
    </aside>
  )
}
