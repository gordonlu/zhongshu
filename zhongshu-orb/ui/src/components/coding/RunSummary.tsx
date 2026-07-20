import type { CodingState } from '../../state/codingReducer'

type SummaryTone = 'neutral' | 'good' | 'warn' | 'bad'

type SummaryItem = {
  label: string
  value: string
  detail?: string
  tone: SummaryTone
}

export function RunSummary({ state }: { state: CodingState }) {
  const items = buildRunSummary(state)

  return (
    <section className="run-summary" aria-label="Run summary">
      {items.map((item) => (
        <div key={item.label} className={`run-summary-item ${item.tone}`}>
          <span>{item.label}</span>
          <strong>{item.value}</strong>
          {item.detail ? <small>{item.detail}</small> : null}
        </div>
      ))}
    </section>
  )
}

export function buildRunSummary(state: CodingState): SummaryItem[] {
  const failedChecks = state.verifications.filter((verification) => !verification.success).length
  const passedChecks = state.verifications.length - failedChecks
  const conflictedWorkers = state.workers.filter((worker) => worker.status === 'conflict').length
  const submittedWorkers = state.workers.filter((worker) => worker.status === 'submitted').length
  const appliedChanges = state.changes.filter((change) => change.status === 'applied').length
  const changedFiles = state.changes.filter((change) => change.changed === true).length
  const hasDroppedContext = Boolean(state.droppedEvidence || state.droppedRecent)
  const contextPressure = state.contextPressure ?? 0
  const needsAttention = failedChecks > 0 || conflictedWorkers > 0 || state.recoveryMessages.length > 0
  const awaitsVerification = submittedWorkers > 0 && !needsAttention
  const reviewReady =
    state.changes.length > 0 &&
    appliedChanges === state.changes.length &&
    state.verifications.length > 0 &&
    failedChecks === 0

  return [
    {
      label: 'Outcome',
      value: needsAttention
        ? 'Needs attention'
        : awaitsVerification
          ? 'Awaiting verification'
          : reviewReady
            ? 'Review ready'
            : state.active
              ? 'Running'
              : 'Standby',
      detail: needsAttention
        ? `${failedChecks} failed checks, ${conflictedWorkers} conflicts`
        : awaitsVerification
          ? `${submittedWorkers} worker submissions`
        : reviewReady
          ? `${changedFiles} changed files`
          : state.risk
            ? `${state.risk} risk`
            : undefined,
      tone: needsAttention ? 'bad' : awaitsVerification ? 'warn' : reviewReady ? 'good' : state.active ? 'warn' : 'neutral',
    },
    {
      label: 'Phase',
      value: state.phase ? `${state.phase.from} -> ${state.phase.to}` : state.sessionId ? state.sessionId : 'waiting',
      detail: state.planStepCount ? `${state.steps.length}/${state.planStepCount} plan steps` : undefined,
      tone: state.phase ? 'warn' : 'neutral',
    },
    {
      label: 'Review',
      value: state.changes.length ? `${appliedChanges}/${state.changes.length} applied` : 'No changes',
      detail: state.changes.length ? `${changedFiles} changed` : undefined,
      tone: state.changes.length && appliedChanges === state.changes.length ? 'good' : 'neutral',
    },
    {
      label: 'Checks',
      value: state.verifications.length ? `${passedChecks}/${state.verifications.length} passed` : 'No checks',
      detail: failedChecks ? `${failedChecks} failed` : undefined,
      tone: failedChecks ? 'bad' : state.verifications.length ? 'good' : 'neutral',
    },
    {
      label: 'Context',
      value: state.contextPressure === undefined ? 'No telemetry' : `${state.contextPressure}%`,
      detail: hasDroppedContext
        ? `dropped ${state.droppedEvidence ?? 0} evidence, ${state.droppedRecent ?? 0} recent`
        : undefined,
      tone: contextPressure >= 90 || hasDroppedContext ? 'bad' : contextPressure >= 75 ? 'warn' : 'neutral',
    },
    {
      label: 'Replay',
      value: state.replay ? 'Available' : 'Not linked',
      detail: state.replay?.replayExecutionId ?? (state.replay?.conversationId ? `conversation ${state.replay.conversationId}` : undefined),
      tone: state.replay ? 'good' : 'neutral',
    },
  ]
}
