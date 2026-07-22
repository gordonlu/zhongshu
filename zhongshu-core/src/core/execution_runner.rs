use std::future::Future;

use crate::agent::{
    ExecutionArtifact, ExecutionGraph, ExecutionGraphError, ExecutionNode,
    ExecutionReconciliationDecision, ExecutionRecoveryReport, NodeExecutionOutcome,
};
use crate::core::{ExecutionGraphStore, ExecutionGraphStoreError, StoredExecutionGraphCheckpoint};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurablePersistencePhase {
    Initialize,
    Admission,
    Outcome,
    Recovery,
    Reconciliation,
}

#[derive(Debug, Clone)]
pub struct DurableExecutionRecovery {
    pub graph: ExecutionGraph,
    pub store_version: u64,
    pub report: ExecutionRecoveryReport,
}

#[derive(Debug)]
pub enum DurableExecutionError {
    Graph(ExecutionGraphError),
    Store {
        phase: DurablePersistencePhase,
        source: ExecutionGraphStoreError,
    },
    PersistenceTask {
        phase: DurablePersistencePhase,
        reason: String,
    },
    DeferredAfterAdmission {
        node_id: String,
    },
    OutcomeProjection {
        node_id: String,
        source: ExecutionGraphError,
    },
}

impl std::fmt::Display for DurableExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Graph(error) => write!(formatter, "execution graph error: {error}"),
            Self::Store { phase, source } => {
                write!(
                    formatter,
                    "{phase:?} checkpoint persistence failed: {source}"
                )
            }
            Self::PersistenceTask { phase, reason } => {
                write!(formatter, "{phase:?} checkpoint task failed: {reason}")
            }
            Self::DeferredAfterAdmission { node_id } => write!(
                formatter,
                "node '{node_id}' deferred after durable admission; its effects require recovery"
            ),
            Self::OutcomeProjection { node_id, source } => write!(
                formatter,
                "node '{node_id}' produced an invalid outcome after durable admission: {source}; its effects require recovery"
            ),
        }
    }
}

impl std::error::Error for DurableExecutionError {}

impl From<ExecutionGraphError> for DurableExecutionError {
    fn from(error: ExecutionGraphError) -> Self {
        Self::Graph(error)
    }
}

/// Executes one ready node across a durable admission boundary.
///
/// The handler is invoked only after the Running transition is saved. If the
/// outcome cannot be saved, the graph remains Running so restart recovery
/// treats the external effect as unknown instead of retrying it.
#[derive(Clone)]
pub struct DurableExecutionRunner<S> {
    store: S,
}

impl<S> DurableExecutionRunner<S>
where
    S: ExecutionGraphStore + Clone + Send + Sync + 'static,
{
    pub fn new(store: S) -> Self {
        Self { store }
    }

    pub async fn initialize(&self, graph: &ExecutionGraph) -> Result<u64, DurableExecutionError> {
        self.save_checkpoint(graph.checkpoint(), 0, DurablePersistencePhase::Initialize)
            .await
    }

    pub async fn recover(
        &self,
        task_id: &str,
    ) -> Result<Option<DurableExecutionRecovery>, DurableExecutionError> {
        let Some(stored) = self.load_checkpoint(task_id).await? else {
            return Ok(None);
        };
        let original_checkpoint = stored.checkpoint.clone();
        let (graph, report) = ExecutionGraph::recover_from_checkpoint(stored.checkpoint)?;
        let store_version = if graph.checkpoint() == original_checkpoint {
            stored.version
        } else {
            self.save_checkpoint(
                graph.checkpoint(),
                stored.version,
                DurablePersistencePhase::Recovery,
            )
            .await?
        };
        Ok(Some(DurableExecutionRecovery {
            graph,
            store_version,
            report,
        }))
    }

    pub async fn execute_node<F, Fut>(
        &self,
        graph: &mut ExecutionGraph,
        store_version: &mut u64,
        node_id: &str,
        handler: F,
    ) -> Result<NodeExecutionOutcome, DurableExecutionError>
    where
        F: FnOnce(ExecutionNode) -> Fut,
        Fut: Future<Output = NodeExecutionOutcome>,
    {
        self.admit_node(graph, store_version, node_id).await?;
        let node = graph
            .node(node_id)
            .cloned()
            .ok_or_else(|| ExecutionGraphError::MissingNode(node_id.into()))?;

        let outcome = handler(node).await;
        self.record_outcome(graph, store_version, node_id, &outcome)
            .await?;
        Ok(outcome)
    }

    pub async fn admit_node(
        &self,
        graph: &mut ExecutionGraph,
        store_version: &mut u64,
        node_id: &str,
    ) -> Result<(), DurableExecutionError> {
        let mut admitted = graph.clone();
        admitted.start_node(node_id)?;
        self.persist_admission(graph, store_version, admitted).await
    }

    pub async fn admit_ready_batch(
        &self,
        graph: &mut ExecutionGraph,
        store_version: &mut u64,
        node_ids: &[String],
    ) -> Result<(), DurableExecutionError> {
        let mut admitted = graph.clone();
        admitted.start_ready_batch(node_ids)?;
        self.persist_admission(graph, store_version, admitted).await
    }

    pub async fn commit_deterministic<F>(
        &self,
        graph: &mut ExecutionGraph,
        store_version: &mut u64,
        transition: F,
    ) -> Result<(), DurableExecutionError>
    where
        F: FnOnce(&mut ExecutionGraph) -> Result<(), ExecutionGraphError>,
    {
        let mut next = graph.clone();
        transition(&mut next)?;
        let next_version = self
            .save_checkpoint(
                next.checkpoint(),
                *store_version,
                DurablePersistencePhase::Outcome,
            )
            .await?;
        *graph = next;
        *store_version = next_version;
        Ok(())
    }

    pub async fn record_outcome(
        &self,
        graph: &mut ExecutionGraph,
        store_version: &mut u64,
        node_id: &str,
        outcome: &NodeExecutionOutcome,
    ) -> Result<(), DurableExecutionError> {
        let mut completed = graph.clone();
        match &outcome {
            NodeExecutionOutcome::Succeeded(artifacts) => {
                completed
                    .complete_node(node_id, artifacts.clone())
                    .map_err(|source| DurableExecutionError::OutcomeProjection {
                        node_id: node_id.into(),
                        source,
                    })?;
            }
            NodeExecutionOutcome::Failed(reason) => {
                completed
                    .fail_node(node_id, reason.clone())
                    .map_err(|source| DurableExecutionError::OutcomeProjection {
                        node_id: node_id.into(),
                        source,
                    })?;
            }
            NodeExecutionOutcome::Cancelled(reason) => {
                completed
                    .cancel_node(node_id, reason.clone())
                    .map_err(|source| DurableExecutionError::OutcomeProjection {
                        node_id: node_id.into(),
                        source,
                    })?;
            }
            NodeExecutionOutcome::Deferred => {
                return Err(DurableExecutionError::DeferredAfterAdmission {
                    node_id: node_id.into(),
                });
            }
        }

        let completed_version = self
            .save_checkpoint(
                completed.checkpoint(),
                *store_version,
                DurablePersistencePhase::Outcome,
            )
            .await?;
        *graph = completed;
        *store_version = completed_version;
        Ok(())
    }

    pub async fn reconcile_node(
        &self,
        graph: &mut ExecutionGraph,
        store_version: &mut u64,
        node_id: &str,
        decision: ExecutionReconciliationDecision,
        reason: impl Into<String>,
        evidence_refs: Vec<String>,
        artifacts: Vec<ExecutionArtifact>,
    ) -> Result<(), DurableExecutionError> {
        let mut reconciled = graph.clone();
        reconciled.reconcile_node(node_id, decision, reason, evidence_refs, artifacts)?;
        let reconciled_version = self
            .save_checkpoint(
                reconciled.checkpoint(),
                *store_version,
                DurablePersistencePhase::Reconciliation,
            )
            .await?;
        *graph = reconciled;
        *store_version = reconciled_version;
        Ok(())
    }

    async fn persist_admission(
        &self,
        graph: &mut ExecutionGraph,
        store_version: &mut u64,
        admitted: ExecutionGraph,
    ) -> Result<(), DurableExecutionError> {
        let admitted_version = self
            .save_checkpoint(
                admitted.checkpoint(),
                *store_version,
                DurablePersistencePhase::Admission,
            )
            .await?;
        *graph = admitted;
        *store_version = admitted_version;
        Ok(())
    }

    async fn save_checkpoint(
        &self,
        checkpoint: crate::agent::ExecutionGraphCheckpoint,
        expected_version: u64,
        phase: DurablePersistencePhase,
    ) -> Result<u64, DurableExecutionError> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.save_graph_cas(&checkpoint, expected_version))
            .await
            .map_err(|error| DurableExecutionError::PersistenceTask {
                phase,
                reason: error.to_string(),
            })?
            .map_err(|source| DurableExecutionError::Store { phase, source })
    }

    async fn load_checkpoint(
        &self,
        task_id: &str,
    ) -> Result<Option<StoredExecutionGraphCheckpoint>, DurableExecutionError> {
        let store = self.store.clone();
        let task_id = task_id.to_string();
        tokio::task::spawn_blocking(move || store.load_graph(&task_id))
            .await
            .map_err(|error| DurableExecutionError::PersistenceTask {
                phase: DurablePersistencePhase::Recovery,
                reason: error.to_string(),
            })?
            .map_err(|source| DurableExecutionError::Store {
                phase: DurablePersistencePhase::Recovery,
                source,
            })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::agent::{ExecutionNodeKind, ExecutionNodeState};
    use crate::core::{Database, OrganizationCheckpointStore};

    fn graph(task_id: &str) -> ExecutionGraph {
        let mut graph = ExecutionGraph::new(task_id).unwrap();
        graph
            .add_node(ExecutionNode::pending(
                "work",
                ExecutionNodeKind::Work,
                "perform external work",
            ))
            .unwrap();
        graph
    }

    #[tokio::test]
    async fn persists_admission_before_handler_and_outcome_after_success() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path().join("durable_runner.db"));
        db.migrate().unwrap();
        let store = OrganizationCheckpointStore::new(db);
        let runner = DurableExecutionRunner::new(store.clone());
        let mut graph = graph("durable-success");
        let mut version = runner.initialize(&graph).await.unwrap();

        let observer = store.clone();
        let outcome = runner
            .execute_node(&mut graph, &mut version, "work", move |node| async move {
                assert_eq!(node.state, ExecutionNodeState::Running);
                let stored = observer.load_graph("durable-success").unwrap().unwrap();
                assert_eq!(stored.version, 2);
                assert_eq!(
                    stored.checkpoint.graph.nodes[0].state,
                    ExecutionNodeState::Running
                );
                NodeExecutionOutcome::Succeeded(Vec::new())
            })
            .await
            .unwrap();

        assert!(matches!(outcome, NodeExecutionOutcome::Succeeded(_)));
        assert_eq!(version, 3);
        assert_eq!(
            graph.node("work").unwrap().state,
            ExecutionNodeState::Succeeded
        );
        let stored = store.load_graph("durable-success").unwrap().unwrap();
        assert_eq!(stored.version, 3);
        assert_eq!(
            stored.checkpoint.graph.nodes[0].state,
            ExecutionNodeState::Succeeded
        );
    }

    #[tokio::test]
    async fn stale_admission_does_not_call_handler_or_mutate_memory_graph() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path().join("stale_admission.db"));
        db.migrate().unwrap();
        let store = OrganizationCheckpointStore::new(db);
        let runner = DurableExecutionRunner::new(store.clone());
        let mut graph = graph("stale-admission");
        let mut version = runner.initialize(&graph).await.unwrap();
        store.save_graph_cas(&graph.checkpoint(), version).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = calls.clone();

        let error = runner
            .execute_node(&mut graph, &mut version, "work", move |_| async move {
                handler_calls.fetch_add(1, Ordering::SeqCst);
                NodeExecutionOutcome::Succeeded(Vec::new())
            })
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            DurableExecutionError::Store {
                phase: DurablePersistencePhase::Admission,
                source: ExecutionGraphStoreError::VersionConflict {
                    expected: 1,
                    actual: 2
                }
            }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(version, 1);
        assert_eq!(
            graph.node("work").unwrap().state,
            ExecutionNodeState::Pending
        );
    }

    #[tokio::test]
    async fn outcome_conflict_leaves_durable_running_for_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outcome_conflict.db");
        let db = Database::new(path.clone());
        db.migrate().unwrap();
        let store = OrganizationCheckpointStore::new(db);
        let runner = DurableExecutionRunner::new(store.clone());
        let mut graph = graph("outcome-conflict");
        let mut version = runner.initialize(&graph).await.unwrap();

        let competing_writer = store.clone();
        let error = runner
            .execute_node(&mut graph, &mut version, "work", move |_| async move {
                let running = competing_writer
                    .load_graph("outcome-conflict")
                    .unwrap()
                    .unwrap();
                competing_writer
                    .save_graph_cas(&running.checkpoint, running.version)
                    .unwrap();
                NodeExecutionOutcome::Succeeded(Vec::new())
            })
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            DurableExecutionError::Store {
                phase: DurablePersistencePhase::Outcome,
                source: ExecutionGraphStoreError::VersionConflict {
                    expected: 2,
                    actual: 3
                }
            }
        ));
        assert_eq!(version, 2);
        assert_eq!(
            graph.node("work").unwrap().state,
            ExecutionNodeState::Running
        );

        let reopened = OrganizationCheckpointStore::new(Database::new(path));
        let stored = reopened.load_graph("outcome-conflict").unwrap().unwrap();
        let (recovered, report) =
            ExecutionGraph::recover_from_checkpoint(stored.checkpoint).unwrap();
        assert_eq!(report.recovery_required_nodes, vec!["work"]);
        assert_eq!(
            recovered.node("work").unwrap().state,
            ExecutionNodeState::RecoveryRequired
        );
    }

    #[tokio::test]
    async fn ready_batch_is_persisted_atomically_before_workers_start() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path().join("durable_batch.db"));
        db.migrate().unwrap();
        let store = OrganizationCheckpointStore::new(db);
        let runner = DurableExecutionRunner::new(store.clone());
        let mut graph = ExecutionGraph::new("durable-batch").unwrap();
        for node_id in ["worker-a", "worker-b"] {
            graph
                .add_node(ExecutionNode::pending(
                    node_id,
                    ExecutionNodeKind::Work,
                    "independent work",
                ))
                .unwrap();
        }
        let mut version = runner.initialize(&graph).await.unwrap();
        let nodes = vec!["worker-a".to_string(), "worker-b".to_string()];

        runner
            .admit_ready_batch(&mut graph, &mut version, &nodes)
            .await
            .unwrap();

        assert_eq!(version, 2);
        let stored = store.load_graph("durable-batch").unwrap().unwrap();
        assert!(stored
            .checkpoint
            .graph
            .nodes
            .iter()
            .all(|node| node.state == ExecutionNodeState::Running));
        for node_id in nodes {
            runner
                .record_outcome(
                    &mut graph,
                    &mut version,
                    &node_id,
                    &NodeExecutionOutcome::Succeeded(Vec::new()),
                )
                .await
                .unwrap();
        }
        assert_eq!(version, 4);
    }

    #[tokio::test]
    async fn recovery_cas_persists_unknown_effect_without_running_handler() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("durable_recovery.db");
        let db = Database::new(path.clone());
        db.migrate().unwrap();
        let store = OrganizationCheckpointStore::new(db);
        let runner = DurableExecutionRunner::new(store.clone());
        let mut graph = graph("durable-recovery");
        let mut version = runner.initialize(&graph).await.unwrap();
        runner
            .admit_node(&mut graph, &mut version, "work")
            .await
            .unwrap();
        drop(runner);
        drop(store);

        let reopened = OrganizationCheckpointStore::new(Database::new(path));
        let recovered = DurableExecutionRunner::new(reopened.clone())
            .recover("durable-recovery")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(recovered.store_version, 3);
        assert_eq!(recovered.report.recovery_required_nodes, vec!["work"]);
        assert_eq!(
            recovered.graph.node("work").unwrap().state,
            ExecutionNodeState::RecoveryRequired
        );
        let stored = reopened.load_graph("durable-recovery").unwrap().unwrap();
        assert_eq!(stored.version, 3);
        assert_eq!(
            stored.checkpoint.graph.nodes[0].state,
            ExecutionNodeState::RecoveryRequired
        );
    }

    #[tokio::test]
    async fn reconciliation_cas_persists_evidence_without_rerunning_handler() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("durable_reconciliation.db");
        let db = Database::new(path.clone());
        db.migrate().unwrap();
        let store = OrganizationCheckpointStore::new(db);
        let runner = DurableExecutionRunner::new(store.clone());
        let mut graph = graph("durable-reconciliation");
        let mut version = runner.initialize(&graph).await.unwrap();
        runner
            .admit_node(&mut graph, &mut version, "work")
            .await
            .unwrap();
        drop(runner);
        drop(store);

        let reopened = OrganizationCheckpointStore::new(Database::new(path));
        let runner = DurableExecutionRunner::new(reopened.clone());
        let recovered = runner
            .recover("durable-reconciliation")
            .await
            .unwrap()
            .unwrap();
        let mut graph = recovered.graph;
        let mut version = recovered.store_version;
        runner
            .reconcile_node(
                &mut graph,
                &mut version,
                "work",
                ExecutionReconciliationDecision::ConfirmedSucceeded,
                "provider receipt confirms the operation committed",
                vec!["provider-receipt:receipt-1".into()],
                vec![ExecutionArtifact {
                    id: "artifact-work-reconciled".into(),
                    producer_node: "work".into(),
                    kind: "reconciled_result".into(),
                    summary: "operation confirmed from provider receipt".into(),
                    evidence_refs: vec!["provider-receipt:receipt-1".into()],
                    uncertainties: Vec::new(),
                }],
            )
            .await
            .unwrap();

        assert_eq!(version, 4);
        assert_eq!(
            graph.node("work").unwrap().state,
            ExecutionNodeState::Succeeded
        );
        let stored = reopened
            .load_graph("durable-reconciliation")
            .unwrap()
            .unwrap();
        assert_eq!(stored.version, 4);
        assert_eq!(stored.checkpoint.graph.reconciliations.len(), 1);
        assert!(reopened.list_unfinished_graphs().unwrap().is_empty());
    }

    #[tokio::test]
    async fn stale_reconciliation_does_not_change_unknown_memory_state() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path().join("stale_reconciliation.db"));
        db.migrate().unwrap();
        let store = OrganizationCheckpointStore::new(db);
        let runner = DurableExecutionRunner::new(store.clone());
        let mut graph = graph("stale-reconciliation");
        let mut version = runner.initialize(&graph).await.unwrap();
        runner
            .admit_node(&mut graph, &mut version, "work")
            .await
            .unwrap();
        let recovered = runner
            .recover("stale-reconciliation")
            .await
            .unwrap()
            .unwrap();
        let mut graph = recovered.graph;
        let mut version = recovered.store_version;
        store.save_graph_cas(&graph.checkpoint(), version).unwrap();

        let error = runner
            .reconcile_node(
                &mut graph,
                &mut version,
                "work",
                ExecutionReconciliationDecision::ConfirmedFailed,
                "provider reports no committed operation",
                vec!["provider-receipt:receipt-2".into()],
                Vec::new(),
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            DurableExecutionError::Store {
                phase: DurablePersistencePhase::Reconciliation,
                source: ExecutionGraphStoreError::VersionConflict {
                    expected: 3,
                    actual: 4
                }
            }
        ));
        assert_eq!(version, 3);
        assert_eq!(
            graph.node("work").unwrap().state,
            ExecutionNodeState::RecoveryRequired
        );
        assert!(graph.snapshot().reconciliations.is_empty());
    }

    #[tokio::test]
    async fn deterministic_transition_is_not_exposed_when_cas_fails() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path().join("deterministic_conflict.db"));
        db.migrate().unwrap();
        let store = OrganizationCheckpointStore::new(db);
        let runner = DurableExecutionRunner::new(store.clone());
        let mut graph = graph("deterministic-conflict");
        let mut version = runner.initialize(&graph).await.unwrap();
        store.save_graph_cas(&graph.checkpoint(), version).unwrap();

        let error = runner
            .commit_deterministic(&mut graph, &mut version, |next| {
                next.cancel_node("work", "policy rejected execution")
            })
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            DurableExecutionError::Store {
                phase: DurablePersistencePhase::Outcome,
                source: ExecutionGraphStoreError::VersionConflict {
                    expected: 1,
                    actual: 2
                }
            }
        ));
        assert_eq!(version, 1);
        assert_eq!(
            graph.node("work").unwrap().state,
            ExecutionNodeState::Pending
        );
    }
}
