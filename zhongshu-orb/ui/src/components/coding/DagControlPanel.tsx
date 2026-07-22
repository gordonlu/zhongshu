import { useState } from 'react'
import type { OrganizationGraphView, OrganizationRecoveryResult } from '../../ipc/events'

type DagControlPanelProps = {
  graphs: OrganizationGraphView[]
  recoveryResults: OrganizationRecoveryResult[]
  onReconcile: (taskId: string, nodeId: string) => void
  onAbandon: (taskId: string, nodeId: string, reason: string) => void
}

export function DagControlPanel({
  graphs,
  recoveryResults,
  onReconcile,
  onAbandon,
}: DagControlPanelProps) {
  const [abandonReasons, setAbandonReasons] = useState<Record<string, string>>({})

  const needsAttention = (view: OrganizationGraphView) =>
    view.graph.nodes.some((node) => node.state === 'recovery_required' || node.state === 'running' || node.state === 'pending')

  const attentionGraphs = graphs.filter(needsAttention)

  if (attentionGraphs.length === 0) {
    return null
  }

  return (
    <div className="dag-control-list">
      {attentionGraphs.map((view) => {
        const graph = view.graph
        const latestResult = [...recoveryResults]
          .reverse()
          .find((result) => result.task_id === graph.task_id)
        return (
          <article className="dag-card" key={graph.task_id}>
            <header>
              <div>
                <strong>{graph.task_id}</strong>
                <span>checkpoint v{view.store_version}</span>
              </div>
              <span>{graph.nodes.filter((node) => node.state === 'recovery_required').length} recovery</span>
            </header>

            <ol className="dag-node-list" aria-label={`Execution graph ${graph.task_id}`}>
              {graph.nodes.map((node) => {
                const dependencies = graph.edges.filter((edge) => edge.to === node.id)
                const artifacts = graph.artifacts.filter((artifact) => artifact.producer_node === node.id)
                const reconciliation = [...graph.reconciliations]
                  .reverse()
                  .find((item) => item.node_id === node.id)
                const reasonKey = `${graph.task_id}:${node.id}`
                const abandonReason = abandonReasons[reasonKey] ?? ''
                return (
                  <li className={`dag-node state-${node.state}`} key={node.id}>
                    <div className="dag-node-heading">
                      <span className="dag-kind">{node.kind}</span>
                      <strong>{node.id}</strong>
                      <span className="dag-state">{node.state.replaceAll('_', ' ')}</span>
                    </div>
                    <p>{node.objective}</p>
                    {dependencies.length > 0 ? (
                      <p className="dag-dependencies">
                        Requires {dependencies.map((edge) => `${edge.from} (${edge.kind})`).join(', ')}
                      </p>
                    ) : null}
                    <div className="dag-evidence-summary">
                      <span>{graph.effect_intents.filter((intent) => intent.node_id === node.id).length} intents</span>
                      <span>{artifacts.length} artifacts</span>
                      <span>{node.executor ?? 'unleased'}</span>
                    </div>
                    {reconciliation ? (
                      <details>
                        <summary>{reconciliation.decision.replaceAll('_', ' ')}</summary>
                        <p>{reconciliation.reason}</p>
                        {reconciliation.evidence_refs.map((reference) => <code key={reference}>{reference}</code>)}
                      </details>
                    ) : null}
                    {node.state === 'recovery_required' ? (
                      <div className="dag-recovery-actions">
                        <button type="button" onClick={() => onReconcile(graph.task_id, node.id)}>
                          Verify external facts
                        </button>
                        <label>
                          <span>Reason for explicit abandonment</span>
                          <input
                            value={abandonReason}
                            onChange={(event) => setAbandonReasons((current) => ({
                              ...current,
                              [reasonKey]: event.target.value,
                            }))}
                            placeholder="Why this unknown effect is being abandoned"
                          />
                        </label>
                        <button
                          type="button"
                          className="danger-action"
                          disabled={abandonReason.trim().length === 0}
                          onClick={() => onAbandon(graph.task_id, node.id, abandonReason.trim())}
                        >
                          Abandon as failed
                        </button>
                      </div>
                    ) : null}
                  </li>
                )
              })}
            </ol>

            {latestResult ? (
              <div className={`dag-recovery-result result-${latestResult.assessment}`} role="status">
                <strong>{latestResult.assessment.replaceAll('_', ' ')}</strong>
                <p>{latestResult.reason}</p>
                {latestResult.executed_cleanup_nodes.length > 0 ? (
                  <p>Cleanup: {latestResult.executed_cleanup_nodes.join(', ')}</p>
                ) : null}
                {latestResult.evidence_refs.length > 0 ? (
                  <details>
                    <summary>{latestResult.evidence_refs.length} evidence references</summary>
                    {latestResult.evidence_refs.map((reference) => <code key={reference}>{reference}</code>)}
                  </details>
                ) : null}
              </div>
            ) : null}
          </article>
        )
      })}
    </div>
  )
}
