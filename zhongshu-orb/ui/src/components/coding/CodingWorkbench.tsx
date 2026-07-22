import type { CodingState } from '../../state/codingReducer'
import { ChangeSetPanel } from './ChangeSetPanel'
import { DagControlPanel } from './DagControlPanel'
import { RunSummary } from './RunSummary'

export const WORKBENCH_RENDER_LIMIT = 80

type CodingWorkbenchProps = {
  state: CodingState
  onReconcileDag?: (taskId: string, nodeId: string) => void
  onAbandonDag?: (taskId: string, nodeId: string, reason: string) => void
}

export function CodingWorkbench({ state, onReconcileDag, onAbandonDag }: CodingWorkbenchProps) {
  const failedChecks = state.verifications.filter((verification) => !verification.success).length
  const activeWorkers = state.workers.filter((worker) => worker.status === 'running').length
  const visibleSteps = latestItems(state.steps)
  const visibleWorkers = latestItems(state.workers)
  const visibleContext = latestItems(state.contextIncluded)
  const visibleChanges = latestItems(state.changes)
  const visibleVerifications = latestItems(state.verifications)
  const visibleRecovery = latestItems(state.recoveryMessages)
  const idle = !state.active && state.workers.length === 0 && state.changes.length === 0 && state.verifications.length === 0 && state.recoveryMessages.length === 0
  const attentionDagCount = state.organizationGraphs.filter((view) =>
    view.graph.nodes.some((node) => node.state === 'recovery_required' || node.state === 'running' || node.state === 'pending')
  ).length
  const runStatus = state.active
    ? 'In progress'
    : state.organization?.status && state.organization.status !== 'standby'
      ? state.organization.status.replaceAll('_', ' ')
      : 'Standby'

  return (
    <aside className="workbench" aria-label="Coding workbench">
      <header className="workbench-header">
        <div>
          <span>Agent run</span>
          <strong>{runStatus}</strong>
        </div>
        {!idle || attentionDagCount > 0 ? (
          <div className="workbench-metrics" aria-label="Run metrics">
            <span>{state.steps.length} steps</span>
            <span>{activeWorkers} active</span>
            <span>{failedChecks} failed</span>
          </div>
        ) : null}
      </header>

      <RunSummary state={state} />

      {!idle || state.organization || state.autoDelegation ? (
        <section className="workbench-section organization-section">
          <div className="section-heading">
            <h2>Organization</h2>
            <span>{state.organization?.status ?? 'standby'}</span>
          </div>
          {state.autoDelegation ? (
            <div className="organization-routing-card">
              <div>
                <span>Automatic routing</span>
                <strong>{state.autoDelegation.strategy.replaceAll('_', ' ')}</strong>
              </div>
              <p>{state.autoDelegation.reason}</p>
              <small>{state.autoDelegation.workerCount} delegated employees</small>
            </div>
          ) : null}
          {state.organization ? (
            <div className="organization-card">
              <div><span>Manager</span><strong>{state.organization.manager}</strong></div>
              <div><span>Flow</span><strong>{state.organization.collaboration.replaceAll('_', ' ')}</strong></div>
              {state.organization.handoff ? (
                <p>{state.organization.handoff.from} → {state.organization.handoff.to}</p>
              ) : null}
              {state.organization.reason ? <p className="organization-reason">{state.organization.reason}</p> : null}
            </div>
          ) : null}
        </section>
      ) : null}

      {attentionDagCount > 0 ? (
        <section className="workbench-section dag-section">
          <div className="section-heading">
            <h2>Durable DAG</h2>
            <span>{attentionDagCount}</span>
          </div>
          <DagControlPanel
            graphs={state.organizationGraphs}
            recoveryResults={state.organizationRecoveryResults}
            onReconcile={onReconcileDag ?? (() => undefined)}
            onAbandon={onAbandonDag ?? (() => undefined)}
          />
        </section>
      ) : null}

      {!idle ? (
        <>
          <section className="workbench-section">
            <div className="section-heading">
              <h2>Plan</h2>
              <span>{state.steps.length}</span>
            </div>
            <HiddenItemCount hidden={hiddenCount(state.steps)} />
            {state.steps.length === 0 ? <p className="muted">Waiting for plan events.</p> : null}
            {visibleSteps.map((step) => (
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
            <HiddenItemCount hidden={hiddenCount(state.workers)} />
            {state.workers.length === 0 ? <p className="muted">No delegated agent activity.</p> : null}
            {visibleWorkers.map((worker) => (
              <div key={worker.taskId} className={`workbench-row worker-row ${worker.status}`}>
                <div className="worker-info">
                  <span className="worker-name">
                    {worker.worker}{worker.role ? ` · ${worker.role}` : ''}
                  </span>
                  <span className={`worker-status-badge ${worker.status === 'completed' || worker.status === 'reported' ? 'success' : worker.status === 'conflict' ? 'fail' : ''}`}>
                    {worker.status}
                  </span>
                  {worker.reason ? (
                    <span className="worker-reason">{worker.reason}</span>
                  ) : null}
                  {worker.ownedFiles.length > 0 ? (
                    <span className="worker-files">{worker.ownedFiles.join(', ')}</span>
                  ) : null}
                </div>
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
            {state.contextIncluded.length > 0 ? (
              <p className="context-tokens">
                ~{state.contextIncluded.reduce((sum, item) => sum + item.estimatedTokens, 0).toLocaleString()} tokens tracked
              </p>
            ) : null}
            {state.droppedEvidence || state.droppedRecent ? (
              <p className="muted">Dropped evidence {state.droppedEvidence ?? 0}, recent {state.droppedRecent ?? 0}</p>
            ) : null}
            <HiddenItemCount hidden={hiddenCount(state.contextIncluded)} />
            {visibleContext.map((item, index) => (
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
            <HiddenItemCount hidden={hiddenCount(state.changes)} />
            <ChangeSetPanel changes={visibleChanges} />
          </section>

          <section className="workbench-section">
            <div className="section-heading">
              <h2>Verification</h2>
              <span>{state.verifications.length}</span>
            </div>
            <HiddenItemCount hidden={hiddenCount(state.verifications)} />
            {state.verifications.length === 0 ? <p className="muted">No checks.</p> : null}
            {visibleVerifications.map((verification, index) => (
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
            <HiddenItemCount hidden={hiddenCount(state.recoveryMessages)} />
            {state.recoveryMessages.length === 0 ? <p className="muted">No recovery feedback.</p> : null}
            {visibleRecovery.map((message, index) => (
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
        </>
      ) : null}
    </aside>
  )
}

function latestItems<T>(items: T[], limit = WORKBENCH_RENDER_LIMIT): T[] {
  if (items.length <= limit) return items
  return items.slice(-limit)
}

function hiddenCount(items: unknown[], limit = WORKBENCH_RENDER_LIMIT): number {
  return Math.max(0, items.length - limit)
}

function HiddenItemCount({ hidden }: { hidden: number }) {
  if (hidden === 0) return null
  return <p className="muted compact-note">{hidden} older items hidden</p>
}
