use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::agent::FileClaimCoordinator;
use crate::agent::{
    ExecutionArtifact, ExecutionEffectExpectation, ExecutionEffectIntent, ExecutionGraph,
    ExecutionNodeKind, ExecutionReconciliationDecision, NodeExecutionOutcome,
};
use crate::core::{DurableExecutionRecovery, DurableExecutionRunner, ExecutionGraphStore};
use crate::integration::DeeplosslessFileReleaseOutcome;
use crate::integration::{DeeplosslessFileClaimFact, DeeplosslessProxy};
use crate::patch::{content_hash, PatchEngine};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalFactAssessment {
    ConfirmedSucceeded,
    ConfirmedFailed,
    Inconclusive,
}

impl ExternalFactAssessment {
    pub fn reconciliation_decision(self) -> Option<ExecutionReconciliationDecision> {
        match self {
            Self::ConfirmedSucceeded => Some(ExecutionReconciliationDecision::ConfirmedSucceeded),
            Self::ConfirmedFailed => Some(ExecutionReconciliationDecision::ConfirmedFailed),
            Self::Inconclusive => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalFactEvidence {
    pub node_id: String,
    pub assessment: ExternalFactAssessment,
    pub reason: String,
    pub evidence_refs: Vec<String>,
    pub artifacts: Vec<ExecutionArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationRecoveryProgress {
    pub evidence: ExternalFactEvidence,
    pub reconciled_node: String,
    pub executed_cleanup_nodes: Vec<String>,
    pub graph: crate::agent::ExecutionGraphSnapshot,
    pub store_version: u64,
}

pub struct WorkspaceEffectFactAdapter {
    workspace_root: PathBuf,
}

impl WorkspaceEffectFactAdapter {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }

    pub fn assess(
        &self,
        graph: &ExecutionGraph,
        node_id: &str,
    ) -> anyhow::Result<ExternalFactEvidence> {
        let intents = graph.effect_intents_for(node_id);
        let workspace_intents = intents
            .iter()
            .filter(|intent| {
                matches!(
                    intent.expectation,
                    ExecutionEffectExpectation::WorkspaceFile { .. }
                )
            })
            .collect::<Vec<_>>();
        if workspace_intents.len() != intents.len() || workspace_intents.is_empty() {
            return Ok(inconclusive(
                node_id,
                "node has no complete workspace effect intent",
                Vec::new(),
            ));
        }

        let mut engine = PatchEngine::new(&self.workspace_root)?;
        let observed_at = observed_at_millis();
        let mut after_matches = 0usize;
        let mut before_matches = 0usize;
        let mut evidence_refs = Vec::new();
        let mut artifacts = Vec::new();
        for (index, intent) in workspace_intents.iter().enumerate() {
            let ExecutionEffectExpectation::WorkspaceFile {
                path,
                before_hash,
                after_hash,
                existed_before,
            } = &intent.expectation
            else {
                unreachable!("workspace intents filtered above")
            };
            let relative = Path::new(path);
            let absolute = engine.workspace_root().join(relative);
            let observed_hash = if absolute.exists() {
                Some(content_hash(&engine.read(relative)?.content))
            } else {
                None
            };
            let after = observed_hash.as_deref() == Some(after_hash.as_str());
            let before = if *existed_before {
                observed_hash.as_deref() == Some(before_hash.as_str())
            } else {
                observed_hash.is_none()
            };
            after_matches += usize::from(after);
            before_matches += usize::from(before);
            let observed = observed_hash.as_deref().unwrap_or("absent");
            let reference =
                format!("workspace:{path}:sha256:{observed}:observed_at_ms:{observed_at}");
            evidence_refs.push(reference.clone());
            artifacts.push(ExecutionArtifact {
                id: format!("reconciled-{node_id}-workspace-{index:03}"),
                producer_node: node_id.into(),
                kind: "reconciled_workspace_file".into(),
                summary: format!("workspace fact for '{path}' matched planned post-state"),
                evidence_refs: vec![reference],
                uncertainties: Vec::new(),
            });
        }

        let total = workspace_intents.len();
        if after_matches == total {
            Ok(ExternalFactEvidence {
                node_id: node_id.into(),
                assessment: ExternalFactAssessment::ConfirmedSucceeded,
                reason: format!(
                    "all {total} workspace file(s) match the persisted Apply post-state"
                ),
                evidence_refs,
                artifacts,
            })
        } else if before_matches == total {
            Ok(ExternalFactEvidence {
                node_id: node_id.into(),
                assessment: ExternalFactAssessment::ConfirmedFailed,
                reason: format!(
                    "all {total} workspace file(s) still match the persisted Apply pre-state"
                ),
                evidence_refs,
                artifacts: Vec::new(),
            })
        } else {
            Ok(inconclusive(
                node_id,
                "workspace contains a partial or divergent Apply state",
                evidence_refs,
            ))
        }
    }
}

#[async_trait]
pub trait FileClaimFactSource: Send + Sync {
    async fn file_claim_facts(&self) -> anyhow::Result<Vec<DeeplosslessFileClaimFact>>;
}

#[async_trait]
impl FileClaimFactSource for DeeplosslessProxy {
    async fn file_claim_facts(&self) -> anyhow::Result<Vec<DeeplosslessFileClaimFact>> {
        DeeplosslessProxy::file_claim_facts(self).await
    }
}

pub struct FileClaimFactAdapter<'a, S> {
    source: &'a S,
}

impl<'a, S> FileClaimFactAdapter<'a, S>
where
    S: FileClaimFactSource,
{
    pub fn new(source: &'a S) -> Self {
        Self { source }
    }

    pub async fn assess(
        &self,
        graph: &ExecutionGraph,
        node_id: &str,
    ) -> anyhow::Result<ExternalFactEvidence> {
        let intents = graph.effect_intents_for(node_id);
        let claim_intents = intents
            .iter()
            .filter(|intent| {
                matches!(
                    intent.expectation,
                    ExecutionEffectExpectation::FileClaim { .. }
                )
            })
            .collect::<Vec<_>>();
        if claim_intents.len() != intents.len() || claim_intents.is_empty() {
            return Ok(inconclusive(
                node_id,
                "node has no complete file-claim effect intent",
                Vec::new(),
            ));
        }
        let expected_active = claim_intents
            .iter()
            .filter_map(|intent| match intent.expectation {
                ExecutionEffectExpectation::FileClaim {
                    expected_active, ..
                } => Some(expected_active),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        if expected_active.len() != 1 {
            return Ok(inconclusive(
                node_id,
                "claim intent mixes active and released expectations",
                Vec::new(),
            ));
        }
        let expected_active = *expected_active
            .iter()
            .next()
            .expect("one expected claim state");
        let facts = self.source.file_claim_facts().await?;
        let observed_at = observed_at_millis();
        let mut desired = 0usize;
        let mut opposite = 0usize;
        let mut evidence_refs = Vec::new();
        for intent in &claim_intents {
            let ExecutionEffectExpectation::FileClaim {
                agent_id,
                file_path,
                operation,
                conv_id,
                ..
            } = &intent.expectation
            else {
                unreachable!("claim intents filtered above")
            };
            let exact = facts.iter().any(|fact| {
                fact.agent_id == *agent_id
                    && fact.file_path == *file_path
                    && fact.operation == *operation
                    && fact.conv_id == *conv_id
            });
            desired += usize::from(exact == expected_active);
            opposite += usize::from(exact != expected_active);
            evidence_refs.push(format!(
                "deeplossless:claim:{agent_id}:{file_path}:{operation}:conv:{conv_id}:active:{exact}:observed_at_ms:{observed_at}"
            ));
        }
        let total = claim_intents.len();
        if desired == total {
            Ok(ExternalFactEvidence {
                node_id: node_id.into(),
                assessment: ExternalFactAssessment::ConfirmedSucceeded,
                reason: format!(
                    "all {total} Deeplossless claim fact(s) match expected_active={expected_active}"
                ),
                evidence_refs: evidence_refs.clone(),
                artifacts: vec![ExecutionArtifact {
                    id: format!("reconciled-{node_id}-claims"),
                    producer_node: node_id.into(),
                    kind: "reconciled_file_claims".into(),
                    summary: format!(
                        "Deeplossless claim registry matched {total} persisted expectation(s)"
                    ),
                    evidence_refs,
                    uncertainties: Vec::new(),
                }],
            })
        } else if opposite == total {
            Ok(ExternalFactEvidence {
                node_id: node_id.into(),
                assessment: ExternalFactAssessment::ConfirmedFailed,
                reason: format!(
                    "all {total} Deeplossless claim fact(s) contradict expected_active={expected_active}"
                ),
                evidence_refs,
                artifacts: Vec::new(),
            })
        } else {
            Ok(inconclusive(
                node_id,
                "Deeplossless claim registry contains a partial expected state",
                evidence_refs,
            ))
        }
    }
}

pub struct MutationRecoveryCoordinator<'a, C> {
    workspace_root: PathBuf,
    external: &'a C,
}

impl<'a, C> MutationRecoveryCoordinator<'a, C>
where
    C: FileClaimCoordinator + FileClaimFactSource,
{
    pub fn new(workspace_root: impl Into<PathBuf>, external: &'a C) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            external,
        }
    }

    pub async fn assess(
        &self,
        graph: &ExecutionGraph,
        node_id: &str,
    ) -> anyhow::Result<ExternalFactEvidence> {
        let node = graph
            .node(node_id)
            .ok_or_else(|| anyhow::anyhow!("recovery node '{node_id}' does not exist"))?;
        match node.kind {
            ExecutionNodeKind::Apply => {
                WorkspaceEffectFactAdapter::new(&self.workspace_root).assess(graph, node_id)
            }
            ExecutionNodeKind::Claim | ExecutionNodeKind::Release => {
                FileClaimFactAdapter::new(self.external)
                    .assess(graph, node_id)
                    .await
            }
            _ => Ok(inconclusive(
                node_id,
                format!(
                    "node kind {:?} has no production external-fact adapter",
                    node.kind
                ),
                Vec::new(),
            )),
        }
    }

    /// Apply one evidence-backed recovery decision and execute only cleanup
    /// and finalization nodes that become ready. Worker or Apply handlers are
    /// never retried by this path.
    pub async fn reconcile_and_continue<S>(
        &self,
        runner: &DurableExecutionRunner<S>,
        recovery: &mut DurableExecutionRecovery,
        node_id: &str,
    ) -> anyhow::Result<MutationRecoveryProgress>
    where
        S: ExecutionGraphStore + Clone + Send + Sync + 'static,
    {
        let evidence = self.assess(&recovery.graph, node_id).await?;
        self.reconcile_evidence_and_continue(runner, recovery, evidence)
            .await
    }

    /// Persist a previously observed conclusive fact assessment. This keeps a
    /// control-plane assessment and its durable decision tied to the same
    /// evidence instead of querying a changing external source twice.
    pub async fn reconcile_evidence_and_continue<S>(
        &self,
        runner: &DurableExecutionRunner<S>,
        recovery: &mut DurableExecutionRecovery,
        evidence: ExternalFactEvidence,
    ) -> anyhow::Result<MutationRecoveryProgress>
    where
        S: ExecutionGraphStore + Clone + Send + Sync + 'static,
    {
        let node_id = evidence.node_id.clone();
        let decision = evidence
            .assessment
            .reconciliation_decision()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "recovery evidence for node '{node_id}' is inconclusive: {}",
                    evidence.reason
                )
            })?;
        runner
            .reconcile_node(
                &mut recovery.graph,
                &mut recovery.store_version,
                &node_id,
                decision,
                evidence.reason.clone(),
                evidence.evidence_refs.clone(),
                evidence.artifacts.clone(),
            )
            .await?;
        let executed_cleanup_nodes = self.continue_after_reconcile(runner, recovery).await?;

        Ok(MutationRecoveryProgress {
            evidence,
            reconciled_node: node_id,
            executed_cleanup_nodes,
            graph: recovery.graph.snapshot(),
            store_version: recovery.store_version,
        })
    }

    /// Explicitly abandon an unknown-effect node. This records a user
    /// decision, not an assertion that no external effect occurred.
    pub async fn abandon_and_continue<S>(
        &self,
        runner: &DurableExecutionRunner<S>,
        recovery: &mut DurableExecutionRecovery,
        node_id: &str,
        decision_id: &str,
        reason: &str,
    ) -> anyhow::Result<MutationRecoveryProgress>
    where
        S: ExecutionGraphStore + Clone + Send + Sync + 'static,
    {
        if decision_id.trim().is_empty() || reason.trim().is_empty() {
            anyhow::bail!("abandon recovery requires a decision id and reason");
        }
        let evidence = ExternalFactEvidence {
            node_id: node_id.into(),
            assessment: ExternalFactAssessment::ConfirmedFailed,
            reason: format!("user abandoned unknown-effect node: {reason}"),
            evidence_refs: vec![format!("user-decision:{decision_id}")],
            artifacts: Vec::new(),
        };
        runner
            .reconcile_node(
                &mut recovery.graph,
                &mut recovery.store_version,
                node_id,
                ExecutionReconciliationDecision::ConfirmedFailed,
                evidence.reason.clone(),
                evidence.evidence_refs.clone(),
                Vec::new(),
            )
            .await?;
        let executed_cleanup_nodes = self.continue_after_reconcile(runner, recovery).await?;
        Ok(MutationRecoveryProgress {
            evidence,
            reconciled_node: node_id.into(),
            executed_cleanup_nodes,
            graph: recovery.graph.snapshot(),
            store_version: recovery.store_version,
        })
    }

    async fn continue_after_reconcile<S>(
        &self,
        runner: &DurableExecutionRunner<S>,
        recovery: &mut DurableExecutionRecovery,
    ) -> anyhow::Result<Vec<String>>
    where
        S: ExecutionGraphStore + Clone + Send + Sync + 'static,
    {
        runner
            .commit_deterministic(&mut recovery.graph, &mut recovery.store_version, |graph| {
                graph.settle_unreachable()?;
                Ok(())
            })
            .await?;

        let mut executed_cleanup_nodes = Vec::new();
        if let Some(release_node) = ready_node_of_kind(&recovery.graph, ExecutionNodeKind::Release)
        {
            self.execute_release_cleanup(runner, recovery, &release_node)
                .await?;
            executed_cleanup_nodes.push(release_node);
            runner
                .commit_deterministic(&mut recovery.graph, &mut recovery.store_version, |graph| {
                    graph.settle_unreachable()?;
                    Ok(())
                })
                .await?;
        }
        if let Some(finalize_node) =
            ready_node_of_kind(&recovery.graph, ExecutionNodeKind::Finalize)
        {
            runner
                .execute_node(
                    &mut recovery.graph,
                    &mut recovery.store_version,
                    &finalize_node,
                    |_| async { NodeExecutionOutcome::Succeeded(Vec::new()) },
                )
                .await?;
            executed_cleanup_nodes.push(finalize_node);
        }

        Ok(executed_cleanup_nodes)
    }

    async fn execute_release_cleanup<S>(
        &self,
        runner: &DurableExecutionRunner<S>,
        recovery: &mut DurableExecutionRecovery,
        release_node: &str,
    ) -> anyhow::Result<()>
    where
        S: ExecutionGraphStore + Clone + Send + Sync + 'static,
    {
        let facts = self.external.file_claim_facts().await?;
        let active_claims = recovery
            .graph
            .snapshot()
            .effect_intents
            .into_iter()
            .filter_map(|intent| match intent.expectation {
                ExecutionEffectExpectation::FileClaim {
                    agent_id,
                    file_path,
                    operation,
                    conv_id,
                    expected_active: true,
                } if facts.iter().any(|fact| {
                    fact.agent_id == agent_id
                        && fact.file_path == file_path
                        && fact.operation == operation
                        && fact.conv_id == conv_id
                }) =>
                {
                    Some((agent_id, file_path, operation, conv_id))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let intents = file_claim_effect_intents(release_node, active_claims.clone(), false);
        if !intents.is_empty() {
            runner
                .commit_deterministic(
                    &mut recovery.graph,
                    &mut recovery.store_version,
                    move |graph| {
                        graph.record_effect_intents(release_node, intents)?;
                        Ok(())
                    },
                )
                .await?;
        }
        runner
            .admit_node(
                &mut recovery.graph,
                &mut recovery.store_version,
                release_node,
            )
            .await?;
        let mut failures = Vec::new();
        for (agent_id, file_path, _, _) in active_claims {
            match self.external.release_file(&agent_id, &file_path).await {
                Ok(DeeplosslessFileReleaseOutcome::Released { .. }) => {}
                Ok(DeeplosslessFileReleaseOutcome::Missing { missing }) => failures.push(format!(
                    "claim '{}' for '{}' disappeared before recovery release: {}",
                    missing.file_path, missing.agent_id, missing.message
                )),
                Err(error) => failures.push(format!(
                    "failed to release recovery claim '{file_path}' for '{agent_id}': {error}"
                )),
            }
        }
        let outcome = if failures.is_empty() {
            NodeExecutionOutcome::Succeeded(vec![ExecutionArtifact {
                id: format!("artifact-{release_node}"),
                producer_node: release_node.into(),
                kind: "recovery_claim_release".into(),
                summary: "all claims observed active during recovery were released".into(),
                evidence_refs: recovery
                    .graph
                    .effect_intents_for(release_node)
                    .into_iter()
                    .map(|intent| format!("effect-intent:{}", intent.id))
                    .collect(),
                uncertainties: Vec::new(),
            }])
        } else {
            NodeExecutionOutcome::Failed(failures.join("; "))
        };
        runner
            .record_outcome(
                &mut recovery.graph,
                &mut recovery.store_version,
                release_node,
                &outcome,
            )
            .await?;
        Ok(())
    }
}

fn ready_node_of_kind(graph: &ExecutionGraph, kind: ExecutionNodeKind) -> Option<String> {
    let ready = graph.ready_node_ids();
    graph
        .snapshot()
        .nodes
        .into_iter()
        .find(|node| node.kind == kind && ready.contains(&node.id))
        .map(|node| node.id)
}

fn inconclusive(
    node_id: &str,
    reason: impl Into<String>,
    evidence_refs: Vec<String>,
) -> ExternalFactEvidence {
    ExternalFactEvidence {
        node_id: node_id.into(),
        assessment: ExternalFactAssessment::Inconclusive,
        reason: reason.into(),
        evidence_refs,
        artifacts: Vec::new(),
    }
}

fn observed_at_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

pub fn workspace_effect_intents(
    node_id: &str,
    plans: &[crate::patch::PatchFileEffectPlan],
) -> Vec<ExecutionEffectIntent> {
    plans
        .iter()
        .enumerate()
        .map(|(index, plan)| ExecutionEffectIntent {
            id: format!("{node_id}:workspace:{index:03}"),
            node_id: node_id.into(),
            expectation: ExecutionEffectExpectation::WorkspaceFile {
                path: plan.path.display().to_string(),
                before_hash: plan.before_hash.clone(),
                after_hash: plan.after_hash.clone(),
                existed_before: plan.existed_before,
            },
        })
        .collect()
}

pub fn file_claim_effect_intents(
    node_id: &str,
    claims: impl IntoIterator<Item = (String, String, String, i64)>,
    expected_active: bool,
) -> Vec<ExecutionEffectIntent> {
    claims
        .into_iter()
        .enumerate()
        .map(
            |(index, (agent_id, file_path, operation, conv_id))| ExecutionEffectIntent {
                id: format!("{node_id}:claim:{index:03}"),
                node_id: node_id.into(),
                expectation: ExecutionEffectExpectation::FileClaim {
                    agent_id,
                    file_path,
                    operation,
                    conv_id,
                    expected_active,
                },
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::agent::{
        ExecutionEdge, ExecutionEdgeKind, ExecutionNode, ExecutionNodeKind, ExecutionNodeState,
    };
    use crate::core::{Database, OrganizationCheckpointStore};
    use crate::integration::{DeeplosslessFileClaimOutcome, DeeplosslessFileReleaseResult};
    use crate::patch::PatchFileEffectPlan;

    fn recovering_graph(
        node_id: &str,
        kind: ExecutionNodeKind,
        intents: Vec<ExecutionEffectIntent>,
    ) -> ExecutionGraph {
        let mut graph = ExecutionGraph::new("reconciliation-facts").unwrap();
        graph
            .add_node(ExecutionNode::pending(node_id, kind, node_id))
            .unwrap();
        graph.record_effect_intents(node_id, intents).unwrap();
        graph.start_node(node_id).unwrap();
        ExecutionGraph::recover_from_checkpoint(graph.checkpoint())
            .unwrap()
            .0
    }

    #[test]
    fn workspace_adapter_distinguishes_before_after_and_divergent_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("copy.txt");
        std::fs::write(&path, "before\n").unwrap();
        let plans = vec![PatchFileEffectPlan {
            path: PathBuf::from("copy.txt"),
            before_hash: content_hash("before\n"),
            after_hash: content_hash("after\n"),
            existed_before: true,
        }];
        let graph = recovering_graph(
            "apply",
            ExecutionNodeKind::Apply,
            workspace_effect_intents("apply", &plans),
        );
        let adapter = WorkspaceEffectFactAdapter::new(directory.path());

        let before = adapter.assess(&graph, "apply").unwrap();
        assert_eq!(before.assessment, ExternalFactAssessment::ConfirmedFailed);
        assert!(before.artifacts.is_empty());

        std::fs::write(&path, "after\n").unwrap();
        let after = adapter.assess(&graph, "apply").unwrap();
        assert_eq!(after.assessment, ExternalFactAssessment::ConfirmedSucceeded);
        assert_eq!(after.artifacts.len(), 1);

        std::fs::write(&path, "different\n").unwrap();
        let divergent = adapter.assess(&graph, "apply").unwrap();
        assert_eq!(divergent.assessment, ExternalFactAssessment::Inconclusive);
    }

    struct MockClaimFacts {
        facts: Mutex<Vec<DeeplosslessFileClaimFact>>,
    }

    #[async_trait]
    impl FileClaimFactSource for MockClaimFacts {
        async fn file_claim_facts(&self) -> anyhow::Result<Vec<DeeplosslessFileClaimFact>> {
            Ok(self.facts.lock().unwrap().clone())
        }
    }

    #[async_trait]
    impl FileClaimCoordinator for MockClaimFacts {
        async fn claim_file(
            &self,
            _agent_id: &str,
            _file_path: &str,
            _operation: &str,
            _conv_id: i64,
        ) -> anyhow::Result<DeeplosslessFileClaimOutcome> {
            anyhow::bail!("recovery coordinator must not acquire new claims")
        }

        async fn release_file(
            &self,
            agent_id: &str,
            file_path: &str,
        ) -> anyhow::Result<DeeplosslessFileReleaseOutcome> {
            let mut facts = self.facts.lock().unwrap();
            let before = facts.len();
            facts.retain(|fact| fact.agent_id != agent_id || fact.file_path != file_path);
            if facts.len() == before {
                Ok(DeeplosslessFileReleaseOutcome::Missing {
                    missing: crate::integration::DeeplosslessFileReleaseMissing {
                        agent_id: agent_id.into(),
                        file_path: file_path.into(),
                        message: "missing".into(),
                    },
                })
            } else {
                Ok(DeeplosslessFileReleaseOutcome::Released {
                    release: DeeplosslessFileReleaseResult {
                        status: "released".into(),
                        file_path: file_path.into(),
                    },
                })
            }
        }
    }

    #[tokio::test]
    async fn claim_adapter_requires_complete_exact_registry_state() {
        let expected = vec![
            ("worker-a".into(), "a.rs".into(), "edit".into(), 7),
            ("worker-b".into(), "b.rs".into(), "edit".into(), 7),
        ];
        let graph = recovering_graph(
            "claim",
            ExecutionNodeKind::Claim,
            file_claim_effect_intents("claim", expected, true),
        );
        let source = MockClaimFacts {
            facts: Mutex::new(vec![DeeplosslessFileClaimFact {
                agent_id: "worker-a".into(),
                file_path: "a.rs".into(),
                operation: "edit".into(),
                conv_id: 7,
            }]),
        };
        let adapter = FileClaimFactAdapter::new(&source);

        let partial = adapter.assess(&graph, "claim").await.unwrap();
        assert_eq!(partial.assessment, ExternalFactAssessment::Inconclusive);

        source
            .facts
            .lock()
            .unwrap()
            .push(DeeplosslessFileClaimFact {
                agent_id: "worker-b".into(),
                file_path: "b.rs".into(),
                operation: "edit".into(),
                conv_id: 7,
            });
        let complete = adapter.assess(&graph, "claim").await.unwrap();
        assert_eq!(
            complete.assessment,
            ExternalFactAssessment::ConfirmedSucceeded
        );
        assert!(!complete.evidence_refs.is_empty());
    }

    #[tokio::test]
    async fn evidence_backed_apply_recovery_releases_claims_and_finalizes() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("copy.txt"), "after\n").unwrap();
        let mut graph = ExecutionGraph::new("mutation-recovery").unwrap();
        for (id, kind) in [
            ("claim", ExecutionNodeKind::Claim),
            ("apply", ExecutionNodeKind::Apply),
            ("release", ExecutionNodeKind::Release),
            ("finalize", ExecutionNodeKind::Finalize),
        ] {
            graph
                .add_node(ExecutionNode::pending(id, kind, id))
                .unwrap();
        }
        graph
            .add_edge(ExecutionEdge {
                from: "apply".into(),
                to: "release".into(),
                kind: ExecutionEdgeKind::Finally,
            })
            .unwrap();
        graph
            .add_edge(ExecutionEdge {
                from: "apply".into(),
                to: "finalize".into(),
                kind: ExecutionEdgeKind::Requires,
            })
            .unwrap();
        graph
            .add_edge(ExecutionEdge {
                from: "release".into(),
                to: "finalize".into(),
                kind: ExecutionEdgeKind::Requires,
            })
            .unwrap();
        graph
            .record_effect_intents(
                "claim",
                file_claim_effect_intents(
                    "claim",
                    vec![("worker".into(), "copy.txt".into(), "edit".into(), 7)],
                    true,
                ),
            )
            .unwrap();
        graph.start_node("claim").unwrap();
        graph.complete_node("claim", Vec::new()).unwrap();
        graph
            .record_effect_intents(
                "apply",
                workspace_effect_intents(
                    "apply",
                    &[PatchFileEffectPlan {
                        path: PathBuf::from("copy.txt"),
                        before_hash: content_hash("before\n"),
                        after_hash: content_hash("after\n"),
                        existed_before: true,
                    }],
                ),
            )
            .unwrap();
        graph.start_node("apply").unwrap();

        let database = Database::new(directory.path().join("recovery.db"));
        database.migrate().unwrap();
        let store = OrganizationCheckpointStore::new(database);
        let runner = DurableExecutionRunner::new(store.clone());
        runner.initialize(&graph).await.unwrap();
        let mut recovery = runner.recover("mutation-recovery").await.unwrap().unwrap();
        let external = MockClaimFacts {
            facts: Mutex::new(vec![DeeplosslessFileClaimFact {
                agent_id: "worker".into(),
                file_path: "copy.txt".into(),
                operation: "edit".into(),
                conv_id: 7,
            }]),
        };
        let coordinator = MutationRecoveryCoordinator::new(directory.path(), &external);

        let progress = coordinator
            .reconcile_and_continue(&runner, &mut recovery, "apply")
            .await
            .unwrap();

        assert_eq!(
            progress.evidence.assessment,
            ExternalFactAssessment::ConfirmedSucceeded
        );
        assert_eq!(progress.executed_cleanup_nodes, vec!["release", "finalize"]);
        assert!(external.facts.lock().unwrap().is_empty());
        assert_eq!(
            recovery.graph.node("apply").unwrap().state,
            ExecutionNodeState::Succeeded
        );
        assert_eq!(
            recovery.graph.node("release").unwrap().state,
            ExecutionNodeState::Succeeded
        );
        assert_eq!(
            recovery.graph.node("finalize").unwrap().state,
            ExecutionNodeState::Succeeded
        );
        let stored = store.load_graph("mutation-recovery").unwrap().unwrap();
        assert_eq!(stored.checkpoint.graph, recovery.graph.snapshot());
        assert!(store.list_unfinished_graphs().unwrap().is_empty());
    }

    #[tokio::test]
    async fn explicit_abandonment_fails_unknown_worker_and_still_releases_claims() {
        let directory = tempfile::tempdir().unwrap();
        let mut graph = ExecutionGraph::new("abandon-recovery").unwrap();
        for (id, kind) in [
            ("claim", ExecutionNodeKind::Claim),
            ("work", ExecutionNodeKind::Propose),
            ("release", ExecutionNodeKind::Release),
            ("finalize", ExecutionNodeKind::Finalize),
        ] {
            graph
                .add_node(ExecutionNode::pending(id, kind, id))
                .unwrap();
        }
        graph
            .add_edge(ExecutionEdge {
                from: "work".into(),
                to: "release".into(),
                kind: ExecutionEdgeKind::Finally,
            })
            .unwrap();
        graph
            .add_edge(ExecutionEdge {
                from: "work".into(),
                to: "finalize".into(),
                kind: ExecutionEdgeKind::Requires,
            })
            .unwrap();
        graph
            .add_edge(ExecutionEdge {
                from: "release".into(),
                to: "finalize".into(),
                kind: ExecutionEdgeKind::Requires,
            })
            .unwrap();
        graph
            .record_effect_intents(
                "claim",
                file_claim_effect_intents(
                    "claim",
                    vec![("worker".into(), "copy.txt".into(), "edit".into(), 8)],
                    true,
                ),
            )
            .unwrap();
        graph.start_node("claim").unwrap();
        graph.complete_node("claim", Vec::new()).unwrap();
        graph.start_node("work").unwrap();

        let database = Database::new(directory.path().join("abandon.db"));
        database.migrate().unwrap();
        let store = OrganizationCheckpointStore::new(database);
        let runner = DurableExecutionRunner::new(store.clone());
        runner.initialize(&graph).await.unwrap();
        let mut recovery = runner.recover("abandon-recovery").await.unwrap().unwrap();
        let external = MockClaimFacts {
            facts: Mutex::new(vec![DeeplosslessFileClaimFact {
                agent_id: "worker".into(),
                file_path: "copy.txt".into(),
                operation: "edit".into(),
                conv_id: 8,
            }]),
        };
        let coordinator = MutationRecoveryCoordinator::new(directory.path(), &external);

        let progress = coordinator
            .abandon_and_continue(
                &runner,
                &mut recovery,
                "work",
                "decision-1",
                "provider result cannot be proven",
            )
            .await
            .unwrap();

        assert_eq!(progress.executed_cleanup_nodes, vec!["release"]);
        assert_eq!(
            recovery.graph.node("work").unwrap().state,
            ExecutionNodeState::Failed
        );
        assert_eq!(
            recovery.graph.node("release").unwrap().state,
            ExecutionNodeState::Succeeded
        );
        assert_eq!(
            recovery.graph.node("finalize").unwrap().state,
            ExecutionNodeState::Skipped
        );
        assert!(external.facts.lock().unwrap().is_empty());
        assert!(store.list_unfinished_graphs().unwrap().is_empty());
    }
}
