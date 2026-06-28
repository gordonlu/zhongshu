import type { CodingState } from '../../state/codingReducer'
import { ChangeSetPanel } from './ChangeSetPanel'

export function CodingWorkbench({ state }: { state: CodingState }) {
  return (
    <aside className="workbench" aria-label="Coding workbench">
      <section className="workbench-section">
        <h2>Plan</h2>
        {state.steps.length === 0 ? <p className="muted">Waiting for plan events.</p> : null}
        {state.steps.map((step) => (
          <div key={step.id} className="workbench-row">
            <span>{step.title}</span>
            <strong>{step.status}</strong>
          </div>
        ))}
      </section>

      <section className="workbench-section">
        <h2>Workers</h2>
        {state.workers.length === 0 ? <p className="muted">No workers.</p> : null}
        {state.workers.map((worker) => (
          <div key={worker.taskId} className={`workbench-row ${worker.status}`}>
            <span>{worker.worker}</span>
            <strong>{worker.status}</strong>
          </div>
        ))}
      </section>

      <section className="workbench-section">
        <h2>Changes</h2>
        <ChangeSetPanel changes={state.changes} />
      </section>

      <section className="workbench-section">
        <h2>Verification</h2>
        {state.verifications.length === 0 ? <p className="muted">No checks.</p> : null}
        {state.verifications.map((verification, index) => (
          <div key={`${verification.command}-${index}`} className={verification.success ? 'check-pass' : 'check-fail'}>
            <span>{verification.success ? 'pass' : 'fail'}</span>
            <code>{verification.command}</code>
          </div>
        ))}
      </section>

      {state.replay ? (
        <section className="workbench-section">
          <h2>Replay</h2>
          <p className="muted">
            {state.replay.replayExecutionId ?? state.replay.conversationId ?? 'available'}
          </p>
        </section>
      ) : null}
    </aside>
  )
}
