//! Append-only execution graph primitives for bounded multi-agent work.
//!
//! Nodes are work units, not persistent agent identities. Agents may be leased
//! to ready nodes, but dependency order, artifacts, and terminal state live in
//! this deterministic graph.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionNodeKind {
    Work,
    Contract,
    Claim,
    Propose,
    Verify,
    Review,
    Decide,
    Apply,
    Release,
    Finalize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionEdgeKind {
    Requires,
    Consumes,
    Validates,
    Supersedes,
    /// Cleanup/finally dependency: the target becomes eligible after the
    /// source reaches any terminal state, not only after success.
    Finally,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionNodeState {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
    /// The process ended while the node was running. Its external effects are
    /// unknown and must be reconciled before any retry is considered.
    RecoveryRequired,
}

impl ExecutionNodeState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded
                | Self::Failed
                | Self::Skipped
                | Self::Cancelled
                | Self::RecoveryRequired
        )
    }

    pub fn is_success(self) -> bool {
        self == Self::Succeeded
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRequirements {
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionNode {
    pub id: String,
    pub kind: ExecutionNodeKind,
    pub objective: String,
    /// The executor selected for this node. This is a lease, not an identity
    /// or an ownership hierarchy.
    pub executor: Option<String>,
    #[serde(default)]
    pub requirements: NodeRequirements,
    pub state: ExecutionNodeState,
}

impl ExecutionNode {
    pub fn pending(
        id: impl Into<String>,
        kind: ExecutionNodeKind,
        objective: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind,
            objective: objective.into(),
            executor: None,
            requirements: NodeRequirements::default(),
            state: ExecutionNodeState::Pending,
        }
    }

    pub fn with_executor(mut self, executor: impl Into<String>) -> Self {
        self.executor = Some(executor.into());
        self
    }

    pub fn with_requirements(mut self, requirements: NodeRequirements) -> Self {
        self.requirements = requirements;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionEdge {
    pub from: String,
    pub to: String,
    pub kind: ExecutionEdgeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionArtifact {
    pub id: String,
    pub producer_node: String,
    pub kind: String,
    pub summary: String,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    #[serde(default)]
    pub uncertainties: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionEffectExpectation {
    WorkspaceFile {
        path: String,
        before_hash: String,
        after_hash: String,
        existed_before: bool,
    },
    FileClaim {
        agent_id: String,
        file_path: String,
        operation: String,
        conv_id: i64,
        expected_active: bool,
    },
}

/// A side-effect expectation persisted before the owning node is admitted.
/// Reconciliation compares external facts against this immutable intent; it
/// must never infer success from the node name or a stale in-memory report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionEffectIntent {
    pub id: String,
    pub node_id: String,
    pub expectation: ExecutionEffectExpectation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionTransition {
    pub sequence: u64,
    pub node_id: String,
    pub from: ExecutionNodeState,
    pub to: ExecutionNodeState,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionReconciliationDecision {
    ConfirmedSucceeded,
    ConfirmedFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionReconciliation {
    pub node_id: String,
    pub decision: ExecutionReconciliationDecision,
    pub reason: String,
    pub evidence_refs: Vec<String>,
    pub transition_sequence: u64,
}

/// A deterministic node handler result. `Deferred` leaves the node pending;
/// the scheduler never invents a terminal result when a handler has no
/// evidence yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeExecutionOutcome {
    Succeeded(Vec<ExecutionArtifact>),
    Failed(String),
    Cancelled(String),
    Deferred,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutionScheduleReport {
    pub executed_nodes: Vec<String>,
    pub skipped_nodes: Vec<String>,
    pub deferred_nodes: Vec<String>,
    /// True when pending nodes remain but a complete pass made no progress.
    pub stalled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionGraphSnapshot {
    pub task_id: String,
    pub nodes: Vec<ExecutionNode>,
    pub edges: Vec<ExecutionEdge>,
    pub artifacts: Vec<ExecutionArtifact>,
    pub transitions: Vec<ExecutionTransition>,
    #[serde(default)]
    pub reconciliations: Vec<ExecutionReconciliation>,
    #[serde(default)]
    pub effect_intents: Vec<ExecutionEffectIntent>,
}

pub const EXECUTION_GRAPH_CHECKPOINT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionGraphCheckpoint {
    pub schema_version: u32,
    pub graph: ExecutionGraphSnapshot,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionRecoveryReport {
    /// Nodes whose in-flight effects became unknown during process loss.
    pub recovery_required_nodes: Vec<String>,
    /// Pending nodes that became unreachable after recovery propagation.
    pub skipped_nodes: Vec<String>,
    /// Nodes eligible for an explicit recovery handler. They are not executed
    /// automatically by checkpoint restoration.
    pub ready_nodes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionGraphError {
    EmptyTaskId,
    EmptyNodeId,
    DuplicateNode(String),
    MissingNode(String),
    DuplicateEdge {
        from: String,
        to: String,
    },
    DuplicateBatchNode(String),
    Cycle {
        from: String,
        to: String,
    },
    MultipleApplyNodes,
    TargetAlreadyStarted(String),
    NodeNotReady(String),
    InvalidTransition {
        node_id: String,
        from: ExecutionNodeState,
        to: ExecutionNodeState,
    },
    DuplicateArtifact(String),
    ArtifactProducerMismatch {
        artifact_id: String,
        expected: String,
        actual: String,
    },
    SchedulerStalled(Vec<String>),
    UnsupportedCheckpointVersion(u32),
    InvalidSnapshot(String),
    NodeNotRecoveryRequired(String),
    EmptyReconciliationReason,
    MissingReconciliationEvidence,
    FailedReconciliationHasArtifacts,
    EmptyEffectIntentId,
    DuplicateEffectIntent(String),
    EffectIntentNodeAlreadyStarted(String),
}

impl std::fmt::Display for ExecutionGraphError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for ExecutionGraphError {}

#[derive(Debug, Clone)]
pub struct ExecutionGraph {
    task_id: String,
    nodes: BTreeMap<String, ExecutionNode>,
    edges: Vec<ExecutionEdge>,
    artifacts: BTreeMap<String, ExecutionArtifact>,
    transitions: Vec<ExecutionTransition>,
    reconciliations: Vec<ExecutionReconciliation>,
    effect_intents: BTreeMap<String, ExecutionEffectIntent>,
    next_sequence: u64,
}

impl ExecutionGraph {
    pub fn new(task_id: impl Into<String>) -> Result<Self, ExecutionGraphError> {
        let task_id = task_id.into();
        if task_id.trim().is_empty() {
            return Err(ExecutionGraphError::EmptyTaskId);
        }
        Ok(Self {
            task_id,
            nodes: BTreeMap::new(),
            edges: Vec::new(),
            artifacts: BTreeMap::new(),
            transitions: Vec::new(),
            reconciliations: Vec::new(),
            effect_intents: BTreeMap::new(),
            next_sequence: 1,
        })
    }

    pub fn add_node(&mut self, node: ExecutionNode) -> Result<(), ExecutionGraphError> {
        if node.id.trim().is_empty() {
            return Err(ExecutionGraphError::EmptyNodeId);
        }
        if self.nodes.contains_key(&node.id) {
            return Err(ExecutionGraphError::DuplicateNode(node.id));
        }
        if node.kind == ExecutionNodeKind::Apply
            && self
                .nodes
                .values()
                .any(|existing| existing.kind == ExecutionNodeKind::Apply)
        {
            return Err(ExecutionGraphError::MultipleApplyNodes);
        }
        self.nodes.insert(node.id.clone(), node);
        Ok(())
    }

    pub fn add_edge(&mut self, edge: ExecutionEdge) -> Result<(), ExecutionGraphError> {
        for node_id in [&edge.from, &edge.to] {
            if !self.nodes.contains_key(node_id) {
                return Err(ExecutionGraphError::MissingNode(node_id.clone()));
            }
        }
        if self
            .nodes
            .get(&edge.to)
            .is_some_and(|node| node.state != ExecutionNodeState::Pending)
        {
            return Err(ExecutionGraphError::TargetAlreadyStarted(edge.to));
        }
        if self
            .edges
            .iter()
            .any(|existing| existing.from == edge.from && existing.to == edge.to)
        {
            return Err(ExecutionGraphError::DuplicateEdge {
                from: edge.from,
                to: edge.to,
            });
        }
        if edge.from == edge.to || self.has_path(&edge.to, &edge.from) {
            return Err(ExecutionGraphError::Cycle {
                from: edge.from,
                to: edge.to,
            });
        }
        self.edges.push(edge);
        Ok(())
    }

    pub fn record_effect_intents(
        &mut self,
        node_id: &str,
        intents: Vec<ExecutionEffectIntent>,
    ) -> Result<(), ExecutionGraphError> {
        let node = self
            .nodes
            .get(node_id)
            .ok_or_else(|| ExecutionGraphError::MissingNode(node_id.into()))?;
        if node.state != ExecutionNodeState::Pending {
            return Err(ExecutionGraphError::EffectIntentNodeAlreadyStarted(
                node_id.into(),
            ));
        }
        let mut ids = BTreeSet::new();
        for intent in &intents {
            if intent.id.trim().is_empty() {
                return Err(ExecutionGraphError::EmptyEffectIntentId);
            }
            if intent.node_id != node_id {
                return Err(ExecutionGraphError::InvalidSnapshot(format!(
                    "effect intent '{}' belongs to node '{}' instead of '{node_id}'",
                    intent.id, intent.node_id
                )));
            }
            if !ids.insert(intent.id.clone()) || self.effect_intents.contains_key(&intent.id) {
                return Err(ExecutionGraphError::DuplicateEffectIntent(
                    intent.id.clone(),
                ));
            }
            validate_effect_expectation(&intent.expectation)?;
        }
        for intent in intents {
            self.effect_intents.insert(intent.id.clone(), intent);
        }
        Ok(())
    }

    pub fn effect_intents_for(&self, node_id: &str) -> Vec<ExecutionEffectIntent> {
        self.effect_intents
            .values()
            .filter(|intent| intent.node_id == node_id)
            .cloned()
            .collect()
    }

    pub fn ready_node_ids(&self) -> Vec<String> {
        self.nodes
            .values()
            .filter(|node| {
                node.state == ExecutionNodeState::Pending
                    && self
                        .edges
                        .iter()
                        .filter(|edge| edge.to == node.id)
                        .all(|edge| {
                            self.nodes.get(&edge.from).is_some_and(|source| {
                                if edge.kind == ExecutionEdgeKind::Finally {
                                    source.state.is_terminal()
                                } else {
                                    source.state.is_success()
                                }
                            })
                        })
            })
            .map(|node| node.id.clone())
            .collect()
    }

    pub fn start_node(&mut self, node_id: &str) -> Result<(), ExecutionGraphError> {
        if !self.ready_node_ids().iter().any(|ready| ready == node_id) {
            return Err(ExecutionGraphError::NodeNotReady(node_id.into()));
        }
        self.transition(node_id, ExecutionNodeState::Running, None)
    }

    /// Atomically admit a set of independent nodes from one ready-set
    /// snapshot. Validation completes before any transition is recorded, so a
    /// bad member cannot leave a partially-started batch.
    pub fn start_ready_batch(&mut self, node_ids: &[String]) -> Result<(), ExecutionGraphError> {
        let ready = self.ready_node_ids().into_iter().collect::<BTreeSet<_>>();
        let mut selected = BTreeSet::new();
        for node_id in node_ids {
            if !self.nodes.contains_key(node_id) {
                return Err(ExecutionGraphError::MissingNode(node_id.clone()));
            }
            if !selected.insert(node_id.clone()) {
                return Err(ExecutionGraphError::DuplicateBatchNode(node_id.clone()));
            }
            if !ready.contains(node_id) {
                return Err(ExecutionGraphError::NodeNotReady(node_id.clone()));
            }
        }
        for node_id in node_ids {
            self.transition(node_id, ExecutionNodeState::Running, None)?;
        }
        Ok(())
    }

    pub fn complete_node(
        &mut self,
        node_id: &str,
        artifacts: Vec<ExecutionArtifact>,
    ) -> Result<(), ExecutionGraphError> {
        self.validate_artifacts(node_id, &artifacts)?;
        self.transition(node_id, ExecutionNodeState::Succeeded, None)?;
        for artifact in artifacts {
            self.artifacts.insert(artifact.id.clone(), artifact);
        }
        Ok(())
    }

    fn validate_artifacts(
        &self,
        node_id: &str,
        artifacts: &[ExecutionArtifact],
    ) -> Result<(), ExecutionGraphError> {
        for artifact in artifacts {
            if artifact.producer_node != node_id {
                return Err(ExecutionGraphError::ArtifactProducerMismatch {
                    artifact_id: artifact.id.clone(),
                    expected: node_id.into(),
                    actual: artifact.producer_node.clone(),
                });
            }
            if self.artifacts.contains_key(&artifact.id) {
                return Err(ExecutionGraphError::DuplicateArtifact(artifact.id.clone()));
            }
        }
        Ok(())
    }

    pub fn fail_node(
        &mut self,
        node_id: &str,
        reason: impl Into<String>,
    ) -> Result<(), ExecutionGraphError> {
        self.transition(node_id, ExecutionNodeState::Failed, Some(reason.into()))
    }

    pub fn cancel_node(
        &mut self,
        node_id: &str,
        reason: impl Into<String>,
    ) -> Result<(), ExecutionGraphError> {
        let state = self
            .nodes
            .get(node_id)
            .ok_or_else(|| ExecutionGraphError::MissingNode(node_id.into()))?
            .state;
        if !matches!(
            state,
            ExecutionNodeState::Pending | ExecutionNodeState::Running
        ) {
            return Err(ExecutionGraphError::InvalidTransition {
                node_id: node_id.into(),
                from: state,
                to: ExecutionNodeState::Cancelled,
            });
        }
        self.set_state(
            node_id,
            state,
            ExecutionNodeState::Cancelled,
            Some(reason.into()),
        )
    }

    pub fn reconcile_node(
        &mut self,
        node_id: &str,
        decision: ExecutionReconciliationDecision,
        reason: impl Into<String>,
        evidence_refs: Vec<String>,
        artifacts: Vec<ExecutionArtifact>,
    ) -> Result<(), ExecutionGraphError> {
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(ExecutionGraphError::EmptyReconciliationReason);
        }
        if evidence_refs.is_empty()
            || evidence_refs
                .iter()
                .any(|evidence| evidence.trim().is_empty())
        {
            return Err(ExecutionGraphError::MissingReconciliationEvidence);
        }
        let state = self
            .nodes
            .get(node_id)
            .ok_or_else(|| ExecutionGraphError::MissingNode(node_id.into()))?
            .state;
        if state != ExecutionNodeState::RecoveryRequired {
            return Err(ExecutionGraphError::NodeNotRecoveryRequired(node_id.into()));
        }
        if decision == ExecutionReconciliationDecision::ConfirmedFailed && !artifacts.is_empty() {
            return Err(ExecutionGraphError::FailedReconciliationHasArtifacts);
        }
        if decision == ExecutionReconciliationDecision::ConfirmedSucceeded {
            self.validate_artifacts(node_id, &artifacts)?;
        }

        let to = match decision {
            ExecutionReconciliationDecision::ConfirmedSucceeded => ExecutionNodeState::Succeeded,
            ExecutionReconciliationDecision::ConfirmedFailed => ExecutionNodeState::Failed,
        };
        let transition_sequence = self.next_sequence;
        self.set_state(
            node_id,
            ExecutionNodeState::RecoveryRequired,
            to,
            Some(reason.clone()),
        )?;
        for artifact in artifacts {
            self.artifacts.insert(artifact.id.clone(), artifact);
        }
        self.reconciliations.push(ExecutionReconciliation {
            node_id: node_id.into(),
            decision,
            reason,
            evidence_refs,
            transition_sequence,
        });
        Ok(())
    }

    /// Mark pending nodes whose prerequisites terminated unsuccessfully. The
    /// operation repeats because skipping one node may make its dependants
    /// unreachable as well.
    pub fn settle_unreachable(&mut self) -> Result<Vec<String>, ExecutionGraphError> {
        let mut skipped = Vec::new();
        loop {
            let next = self.nodes.values().find_map(|node| {
                if node.state != ExecutionNodeState::Pending {
                    return None;
                }
                self.edges
                    .iter()
                    .filter(|edge| edge.to == node.id)
                    .any(|edge| {
                        edge.kind != ExecutionEdgeKind::Finally
                            && self.nodes.get(&edge.from).is_some_and(|source| {
                                source.state.is_terminal()
                                    && source.state != ExecutionNodeState::RecoveryRequired
                                    && !source.state.is_success()
                            })
                    })
                    .then(|| node.id.clone())
            });
            let Some(node_id) = next else {
                break;
            };
            self.set_state(
                &node_id,
                ExecutionNodeState::Pending,
                ExecutionNodeState::Skipped,
                Some("a prerequisite did not succeed".into()),
            )?;
            skipped.push(node_id);
        }
        Ok(skipped)
    }

    pub fn node(&self, node_id: &str) -> Option<&ExecutionNode> {
        self.nodes.get(node_id)
    }

    pub fn snapshot(&self) -> ExecutionGraphSnapshot {
        ExecutionGraphSnapshot {
            task_id: self.task_id.clone(),
            nodes: self.nodes.values().cloned().collect(),
            edges: self.edges.clone(),
            artifacts: self.artifacts.values().cloned().collect(),
            transitions: self.transitions.clone(),
            reconciliations: self.reconciliations.clone(),
            effect_intents: self.effect_intents.values().cloned().collect(),
        }
    }

    pub fn checkpoint(&self) -> ExecutionGraphCheckpoint {
        ExecutionGraphCheckpoint {
            schema_version: EXECUTION_GRAPH_CHECKPOINT_VERSION,
            graph: self.snapshot(),
        }
    }

    /// Restore an append-only graph checkpoint without replaying node side
    /// effects. Any node left Running is marked RecoveryRequired because the
    /// process cannot know whether its external operation committed.
    pub fn recover_from_checkpoint(
        checkpoint: ExecutionGraphCheckpoint,
    ) -> Result<(Self, ExecutionRecoveryReport), ExecutionGraphError> {
        if checkpoint.schema_version != EXECUTION_GRAPH_CHECKPOINT_VERSION {
            return Err(ExecutionGraphError::UnsupportedCheckpointVersion(
                checkpoint.schema_version,
            ));
        }
        let mut graph = Self::validated_snapshot(checkpoint.graph)?;
        let recovery_required_nodes = graph
            .nodes
            .values()
            .filter(|node| node.state == ExecutionNodeState::Running)
            .map(|node| node.id.clone())
            .collect::<Vec<_>>();
        for node_id in &recovery_required_nodes {
            graph.set_state(
                node_id,
                ExecutionNodeState::Running,
                ExecutionNodeState::RecoveryRequired,
                Some("process ended while node was running; external effects are unknown".into()),
            )?;
        }
        let skipped_nodes = graph.settle_unreachable()?;
        let ready_nodes = graph.ready_node_ids();
        Ok((
            graph,
            ExecutionRecoveryReport {
                recovery_required_nodes,
                skipped_nodes,
                ready_nodes,
            },
        ))
    }

    fn validated_snapshot(snapshot: ExecutionGraphSnapshot) -> Result<Self, ExecutionGraphError> {
        let mut topology = Self::new(snapshot.task_id.clone())?;
        for node in &snapshot.nodes {
            topology.add_node(ExecutionNode {
                id: node.id.clone(),
                kind: node.kind,
                objective: node.objective.clone(),
                executor: node.executor.clone(),
                requirements: node.requirements.clone(),
                state: ExecutionNodeState::Pending,
            })?;
        }
        for edge in &snapshot.edges {
            topology.add_edge(edge.clone())?;
        }

        let mut replayed = topology
            .nodes
            .keys()
            .map(|node_id| (node_id.clone(), ExecutionNodeState::Pending))
            .collect::<BTreeMap<_, _>>();
        let mut expected_sequence = 1;
        for transition in &snapshot.transitions {
            if transition.sequence != expected_sequence {
                return Err(ExecutionGraphError::InvalidSnapshot(format!(
                    "transition sequence {} is not expected sequence {expected_sequence}",
                    transition.sequence
                )));
            }
            expected_sequence += 1;
            let state = replayed.get_mut(&transition.node_id).ok_or_else(|| {
                ExecutionGraphError::InvalidSnapshot(format!(
                    "transition references missing node '{}'",
                    transition.node_id
                ))
            })?;
            if *state != transition.from
                || !valid_snapshot_transition(transition.from, transition.to)
            {
                return Err(ExecutionGraphError::InvalidSnapshot(format!(
                    "invalid transition replay for node '{}' from {:?} to {:?}",
                    transition.node_id, transition.from, transition.to
                )));
            }
            *state = transition.to;
        }
        for node in &snapshot.nodes {
            if replayed.get(&node.id).copied() != Some(node.state) {
                return Err(ExecutionGraphError::InvalidSnapshot(format!(
                    "node '{}' state does not match its transition history",
                    node.id
                )));
            }
        }

        let mut reconciliation_sequences = BTreeSet::new();
        for reconciliation in &snapshot.reconciliations {
            if reconciliation.reason.trim().is_empty()
                || reconciliation.evidence_refs.is_empty()
                || reconciliation
                    .evidence_refs
                    .iter()
                    .any(|evidence| evidence.trim().is_empty())
            {
                return Err(ExecutionGraphError::InvalidSnapshot(format!(
                    "reconciliation for node '{}' has no usable evidence",
                    reconciliation.node_id
                )));
            }
            if !reconciliation_sequences.insert(reconciliation.transition_sequence) {
                return Err(ExecutionGraphError::InvalidSnapshot(format!(
                    "duplicate reconciliation transition sequence {}",
                    reconciliation.transition_sequence
                )));
            }
            let transition = snapshot
                .transitions
                .iter()
                .find(|transition| transition.sequence == reconciliation.transition_sequence)
                .ok_or_else(|| {
                    ExecutionGraphError::InvalidSnapshot(format!(
                        "reconciliation for node '{}' references missing transition {}",
                        reconciliation.node_id, reconciliation.transition_sequence
                    ))
                })?;
            let expected_to = match reconciliation.decision {
                ExecutionReconciliationDecision::ConfirmedSucceeded => {
                    ExecutionNodeState::Succeeded
                }
                ExecutionReconciliationDecision::ConfirmedFailed => ExecutionNodeState::Failed,
            };
            if transition.node_id != reconciliation.node_id
                || transition.from != ExecutionNodeState::RecoveryRequired
                || transition.to != expected_to
            {
                return Err(ExecutionGraphError::InvalidSnapshot(format!(
                    "reconciliation for node '{}' does not match transition {}",
                    reconciliation.node_id, reconciliation.transition_sequence
                )));
            }
        }
        for transition in snapshot
            .transitions
            .iter()
            .filter(|transition| transition.from == ExecutionNodeState::RecoveryRequired)
        {
            if !reconciliation_sequences.contains(&transition.sequence) {
                return Err(ExecutionGraphError::InvalidSnapshot(format!(
                    "recovery resolution transition {} has no reconciliation evidence",
                    transition.sequence
                )));
            }
        }

        let mut artifacts = BTreeMap::new();
        for artifact in &snapshot.artifacts {
            let producer_state =
                replayed
                    .get(&artifact.producer_node)
                    .copied()
                    .ok_or_else(|| {
                        ExecutionGraphError::InvalidSnapshot(format!(
                            "artifact '{}' references missing producer '{}'",
                            artifact.id, artifact.producer_node
                        ))
                    })?;
            if producer_state != ExecutionNodeState::Succeeded {
                return Err(ExecutionGraphError::InvalidSnapshot(format!(
                    "artifact '{}' producer '{}' did not succeed",
                    artifact.id, artifact.producer_node
                )));
            }
            if artifacts
                .insert(artifact.id.clone(), artifact.clone())
                .is_some()
            {
                return Err(ExecutionGraphError::DuplicateArtifact(artifact.id.clone()));
            }
        }

        let mut effect_intents = BTreeMap::new();
        for intent in &snapshot.effect_intents {
            if intent.id.trim().is_empty() {
                return Err(ExecutionGraphError::EmptyEffectIntentId);
            }
            if !replayed.contains_key(&intent.node_id) {
                return Err(ExecutionGraphError::InvalidSnapshot(format!(
                    "effect intent '{}' references missing node '{}'",
                    intent.id, intent.node_id
                )));
            }
            validate_effect_expectation(&intent.expectation)?;
            if effect_intents
                .insert(intent.id.clone(), intent.clone())
                .is_some()
            {
                return Err(ExecutionGraphError::DuplicateEffectIntent(
                    intent.id.clone(),
                ));
            }
        }

        Ok(Self {
            task_id: snapshot.task_id,
            nodes: snapshot
                .nodes
                .into_iter()
                .map(|node| (node.id.clone(), node))
                .collect(),
            edges: snapshot.edges,
            artifacts,
            transitions: snapshot.transitions,
            reconciliations: snapshot.reconciliations,
            effect_intents,
            next_sequence: expected_sequence,
        })
    }

    /// Consume the ready set until the graph reaches a terminal state or all
    /// currently ready nodes defer. The handler is called only for nodes whose
    /// dependencies are satisfied. It may perform the real gate operation or
    /// project an already-observed result, but it must return explicit evidence
    /// for success or failure.
    pub fn run_ready<F>(
        &mut self,
        mut handler: F,
    ) -> Result<ExecutionScheduleReport, ExecutionGraphError>
    where
        F: FnMut(&ExecutionNode) -> NodeExecutionOutcome,
    {
        let mut report = ExecutionScheduleReport::default();
        loop {
            report.skipped_nodes.extend(self.settle_unreachable()?);
            let ready = self.ready_node_ids();
            if ready.is_empty() {
                break;
            }

            let mut progressed = false;
            let mut deferred = Vec::new();
            for node_id in ready {
                let node = self
                    .node(&node_id)
                    .cloned()
                    .ok_or_else(|| ExecutionGraphError::MissingNode(node_id.clone()))?;
                match handler(&node) {
                    NodeExecutionOutcome::Deferred => deferred.push(node_id),
                    NodeExecutionOutcome::Succeeded(artifacts) => {
                        // Validate before the node starts so a malformed
                        // handler result cannot strand it in Running.
                        self.validate_artifacts(&node_id, &artifacts)?;
                        self.start_node(&node_id)?;
                        self.complete_node(&node_id, artifacts)?;
                        report.executed_nodes.push(node_id);
                        progressed = true;
                    }
                    NodeExecutionOutcome::Failed(reason) => {
                        self.start_node(&node_id)?;
                        self.fail_node(&node_id, reason)?;
                        report.executed_nodes.push(node_id);
                        progressed = true;
                    }
                    NodeExecutionOutcome::Cancelled(reason) => {
                        self.cancel_node(&node_id, reason)?;
                        report.executed_nodes.push(node_id);
                        progressed = true;
                    }
                }
            }

            if !progressed {
                report.deferred_nodes = deferred;
                report.stalled = true;
                break;
            }
        }
        Ok(report)
    }

    fn transition(
        &mut self,
        node_id: &str,
        to: ExecutionNodeState,
        reason: Option<String>,
    ) -> Result<(), ExecutionGraphError> {
        let from = self
            .nodes
            .get(node_id)
            .ok_or_else(|| ExecutionGraphError::MissingNode(node_id.into()))?
            .state;
        let valid = matches!(
            (from, to),
            (ExecutionNodeState::Pending, ExecutionNodeState::Running)
                | (ExecutionNodeState::Running, ExecutionNodeState::Succeeded)
                | (ExecutionNodeState::Running, ExecutionNodeState::Failed)
        );
        if !valid {
            return Err(ExecutionGraphError::InvalidTransition {
                node_id: node_id.into(),
                from,
                to,
            });
        }
        self.set_state(node_id, from, to, reason)
    }

    fn set_state(
        &mut self,
        node_id: &str,
        from: ExecutionNodeState,
        to: ExecutionNodeState,
        reason: Option<String>,
    ) -> Result<(), ExecutionGraphError> {
        let node = self
            .nodes
            .get_mut(node_id)
            .ok_or_else(|| ExecutionGraphError::MissingNode(node_id.into()))?;
        if node.state != from {
            return Err(ExecutionGraphError::InvalidTransition {
                node_id: node_id.into(),
                from: node.state,
                to,
            });
        }
        node.state = to;
        self.transitions.push(ExecutionTransition {
            sequence: self.next_sequence,
            node_id: node_id.into(),
            from,
            to,
            reason,
        });
        self.next_sequence += 1;
        Ok(())
    }

    fn has_path(&self, from: &str, to: &str) -> bool {
        let mut queue = VecDeque::from([from.to_string()]);
        let mut visited = BTreeSet::new();
        while let Some(current) = queue.pop_front() {
            if current == to {
                return true;
            }
            if !visited.insert(current.clone()) {
                continue;
            }
            for edge in self.edges.iter().filter(|edge| edge.from == current) {
                queue.push_back(edge.to.clone());
            }
        }
        false
    }
}

fn validate_effect_expectation(
    expectation: &ExecutionEffectExpectation,
) -> Result<(), ExecutionGraphError> {
    let invalid = match expectation {
        ExecutionEffectExpectation::WorkspaceFile {
            path,
            before_hash,
            after_hash,
            ..
        } => {
            path.trim().is_empty() || before_hash.trim().is_empty() || after_hash.trim().is_empty()
        }
        ExecutionEffectExpectation::FileClaim {
            agent_id,
            file_path,
            operation,
            conv_id,
            ..
        } => {
            agent_id.trim().is_empty()
                || file_path.trim().is_empty()
                || operation.trim().is_empty()
                || *conv_id <= 0
        }
    };
    if invalid {
        return Err(ExecutionGraphError::InvalidSnapshot(
            "effect intent contains an invalid expectation".into(),
        ));
    }
    Ok(())
}

fn valid_snapshot_transition(from: ExecutionNodeState, to: ExecutionNodeState) -> bool {
    matches!(
        (from, to),
        (ExecutionNodeState::Pending, ExecutionNodeState::Running)
            | (ExecutionNodeState::Running, ExecutionNodeState::Succeeded)
            | (ExecutionNodeState::Running, ExecutionNodeState::Failed)
            | (
                ExecutionNodeState::Pending | ExecutionNodeState::Running,
                ExecutionNodeState::Cancelled
            )
            | (ExecutionNodeState::Pending, ExecutionNodeState::Skipped)
            | (
                ExecutionNodeState::Running,
                ExecutionNodeState::RecoveryRequired
            )
            | (
                ExecutionNodeState::RecoveryRequired,
                ExecutionNodeState::Succeeded | ExecutionNodeState::Failed
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(from: &str, to: &str) -> ExecutionEdge {
        ExecutionEdge {
            from: from.into(),
            to: to.into(),
            kind: ExecutionEdgeKind::Requires,
        }
    }

    #[test]
    fn independent_nodes_are_ready_together_and_join_after_success() {
        let mut graph = ExecutionGraph::new("task").unwrap();
        for id in ["search-a", "search-b", "decide"] {
            graph
                .add_node(ExecutionNode::pending(id, ExecutionNodeKind::Work, id))
                .unwrap();
        }
        graph.add_edge(edge("search-a", "decide")).unwrap();
        graph.add_edge(edge("search-b", "decide")).unwrap();

        assert_eq!(graph.ready_node_ids(), vec!["search-a", "search-b"]);
        graph.start_node("search-a").unwrap();
        graph.complete_node("search-a", Vec::new()).unwrap();
        assert_eq!(graph.ready_node_ids(), vec!["search-b"]);
        graph.start_node("search-b").unwrap();
        graph.complete_node("search-b", Vec::new()).unwrap();
        assert_eq!(graph.ready_node_ids(), vec!["decide"]);
    }

    #[test]
    fn rejects_cycles_and_a_second_apply_node() {
        let mut graph = ExecutionGraph::new("task").unwrap();
        graph
            .add_node(ExecutionNode::pending("a", ExecutionNodeKind::Work, "a"))
            .unwrap();
        graph
            .add_node(ExecutionNode::pending("b", ExecutionNodeKind::Apply, "b"))
            .unwrap();
        graph.add_edge(edge("a", "b")).unwrap();

        assert!(matches!(
            graph.add_edge(edge("b", "a")),
            Err(ExecutionGraphError::Cycle { .. })
        ));
        assert_eq!(
            graph.add_node(ExecutionNode::pending(
                "apply-again",
                ExecutionNodeKind::Apply,
                "apply"
            )),
            Err(ExecutionGraphError::MultipleApplyNodes)
        );
    }

    #[test]
    fn failure_skips_all_unreachable_dependants_without_retrying_nodes() {
        let mut graph = ExecutionGraph::new("task").unwrap();
        for id in ["work", "decide", "finalize"] {
            graph
                .add_node(ExecutionNode::pending(id, ExecutionNodeKind::Work, id))
                .unwrap();
        }
        graph.add_edge(edge("work", "decide")).unwrap();
        graph.add_edge(edge("decide", "finalize")).unwrap();
        graph.start_node("work").unwrap();
        graph.fail_node("work", "provider failed").unwrap();

        assert_eq!(
            graph.settle_unreachable().unwrap(),
            vec!["decide", "finalize"]
        );
        assert!(graph.ready_node_ids().is_empty());
        assert_eq!(
            graph.node("finalize").unwrap().state,
            ExecutionNodeState::Skipped
        );
    }

    #[test]
    fn completion_records_typed_artifacts_and_append_only_transitions() {
        let mut graph = ExecutionGraph::new("task").unwrap();
        graph
            .add_node(ExecutionNode::pending(
                "search",
                ExecutionNodeKind::Work,
                "search",
            ))
            .unwrap();
        graph.start_node("search").unwrap();
        graph
            .complete_node(
                "search",
                vec![ExecutionArtifact {
                    id: "finding-1".into(),
                    producer_node: "search".into(),
                    kind: "finding".into(),
                    summary: "new evidence".into(),
                    evidence_refs: vec!["file:src/lib.rs".into()],
                    uncertainties: Vec::new(),
                }],
            )
            .unwrap();

        let snapshot = graph.snapshot();
        assert_eq!(snapshot.artifacts.len(), 1);
        assert_eq!(snapshot.transitions.len(), 2);
        assert_eq!(snapshot.transitions[0].sequence, 1);
        assert_eq!(snapshot.transitions[1].sequence, 2);
    }

    #[test]
    fn effect_intent_is_immutable_after_admission_and_survives_replay() {
        let mut graph = ExecutionGraph::new("task").unwrap();
        graph
            .add_node(ExecutionNode::pending(
                "apply",
                ExecutionNodeKind::Apply,
                "apply",
            ))
            .unwrap();
        let intent = ExecutionEffectIntent {
            id: "apply:file:copy.txt".into(),
            node_id: "apply".into(),
            expectation: ExecutionEffectExpectation::WorkspaceFile {
                path: "copy.txt".into(),
                before_hash: "before".into(),
                after_hash: "after".into(),
                existed_before: true,
            },
        };
        graph
            .record_effect_intents("apply", vec![intent.clone()])
            .unwrap();
        graph.start_node("apply").unwrap();

        assert_eq!(
            graph.record_effect_intents("apply", vec![intent]),
            Err(ExecutionGraphError::EffectIntentNodeAlreadyStarted(
                "apply".into()
            ))
        );

        let (replayed, report) =
            ExecutionGraph::recover_from_checkpoint(graph.checkpoint()).unwrap();
        assert_eq!(report.recovery_required_nodes, vec!["apply"]);
        assert_eq!(replayed.effect_intents_for("apply").len(), 1);
        assert_eq!(replayed.snapshot().effect_intents.len(), 1);
    }

    #[test]
    fn finally_edge_makes_cleanup_ready_after_failure() {
        let mut graph = ExecutionGraph::new("task").unwrap();
        graph
            .add_node(ExecutionNode::pending(
                "apply",
                ExecutionNodeKind::Apply,
                "apply",
            ))
            .unwrap();
        graph
            .add_node(ExecutionNode::pending(
                "release",
                ExecutionNodeKind::Finalize,
                "release claims",
            ))
            .unwrap();
        graph
            .add_edge(ExecutionEdge {
                from: "apply".into(),
                to: "release".into(),
                kind: ExecutionEdgeKind::Finally,
            })
            .unwrap();

        graph.start_node("apply").unwrap();
        graph.fail_node("apply", "write failed").unwrap();

        assert_eq!(graph.ready_node_ids(), vec!["release"]);
        assert!(graph.settle_unreachable().unwrap().is_empty());
    }

    #[test]
    fn ready_scheduler_propagates_failure_and_still_runs_cleanup() {
        let mut graph = ExecutionGraph::new("task").unwrap();
        for (id, kind) in [
            ("work", ExecutionNodeKind::Work),
            ("apply", ExecutionNodeKind::Apply),
            ("release", ExecutionNodeKind::Release),
            ("finalize", ExecutionNodeKind::Finalize),
        ] {
            graph
                .add_node(ExecutionNode::pending(id, kind, id))
                .unwrap();
        }
        graph.add_edge(edge("work", "apply")).unwrap();
        graph
            .add_edge(ExecutionEdge {
                from: "apply".into(),
                to: "release".into(),
                kind: ExecutionEdgeKind::Finally,
            })
            .unwrap();
        graph.add_edge(edge("apply", "finalize")).unwrap();
        graph.add_edge(edge("release", "finalize")).unwrap();

        let report = graph
            .run_ready(|node| match node.id.as_str() {
                "work" => NodeExecutionOutcome::Failed("worker failed".into()),
                "release" => NodeExecutionOutcome::Succeeded(Vec::new()),
                _ => NodeExecutionOutcome::Deferred,
            })
            .unwrap();

        assert!(!report.stalled);
        assert_eq!(report.executed_nodes, vec!["work", "release"]);
        assert_eq!(
            graph.node("apply").unwrap().state,
            ExecutionNodeState::Skipped
        );
        assert_eq!(
            graph.node("release").unwrap().state,
            ExecutionNodeState::Succeeded
        );
        assert_eq!(
            graph.node("finalize").unwrap().state,
            ExecutionNodeState::Skipped
        );
    }

    #[test]
    fn ready_scheduler_reports_stall_without_fabricating_progress() {
        let mut graph = ExecutionGraph::new("task").unwrap();
        graph
            .add_node(ExecutionNode::pending(
                "external",
                ExecutionNodeKind::Work,
                "wait for external evidence",
            ))
            .unwrap();

        let report = graph.run_ready(|_| NodeExecutionOutcome::Deferred).unwrap();

        assert!(report.stalled);
        assert_eq!(report.deferred_nodes, vec!["external"]);
        assert!(report.executed_nodes.is_empty());
        assert!(graph.snapshot().transitions.is_empty());
        assert_eq!(
            graph.node("external").unwrap().state,
            ExecutionNodeState::Pending
        );
    }

    #[test]
    fn ready_scheduler_rejects_bad_artifact_before_starting_node() {
        let mut graph = ExecutionGraph::new("task").unwrap();
        graph
            .add_node(ExecutionNode::pending(
                "work",
                ExecutionNodeKind::Work,
                "produce evidence",
            ))
            .unwrap();

        let error = graph
            .run_ready(|_| {
                NodeExecutionOutcome::Succeeded(vec![ExecutionArtifact {
                    id: "artifact".into(),
                    producer_node: "different-node".into(),
                    kind: "evidence".into(),
                    summary: "invalid producer".into(),
                    evidence_refs: Vec::new(),
                    uncertainties: Vec::new(),
                }])
            })
            .unwrap_err();

        assert!(matches!(
            error,
            ExecutionGraphError::ArtifactProducerMismatch { .. }
        ));
        assert_eq!(
            graph.node("work").unwrap().state,
            ExecutionNodeState::Pending
        );
        assert!(graph.snapshot().transitions.is_empty());
    }

    #[test]
    fn ready_batch_admission_is_atomic() {
        let mut graph = ExecutionGraph::new("task").unwrap();
        for id in ["first", "dependent"] {
            graph
                .add_node(ExecutionNode::pending(id, ExecutionNodeKind::Work, id))
                .unwrap();
        }
        graph.add_edge(edge("first", "dependent")).unwrap();

        let error = graph
            .start_ready_batch(&["first".into(), "dependent".into()])
            .unwrap_err();

        assert_eq!(error, ExecutionGraphError::NodeNotReady("dependent".into()));
        assert_eq!(
            graph.node("first").unwrap().state,
            ExecutionNodeState::Pending
        );
        assert_eq!(
            graph.node("dependent").unwrap().state,
            ExecutionNodeState::Pending
        );
        assert!(graph.snapshot().transitions.is_empty());
    }

    #[test]
    fn checkpoint_recovery_marks_running_node_unknown_and_holds_dependants() {
        let mut graph = ExecutionGraph::new("task").unwrap();
        graph
            .add_node(ExecutionNode::pending(
                "work",
                ExecutionNodeKind::Work,
                "external work",
            ))
            .unwrap();
        graph
            .add_node(ExecutionNode::pending(
                "decide",
                ExecutionNodeKind::Decide,
                "decide",
            ))
            .unwrap();
        graph.add_edge(edge("work", "decide")).unwrap();
        graph.start_node("work").unwrap();
        let encoded = serde_json::to_string(&graph.checkpoint()).unwrap();
        let checkpoint = serde_json::from_str(&encoded).unwrap();

        let (recovered, report) = ExecutionGraph::recover_from_checkpoint(checkpoint).unwrap();

        assert_eq!(report.recovery_required_nodes, vec!["work"]);
        assert!(report.skipped_nodes.is_empty());
        assert!(report.ready_nodes.is_empty());
        assert_eq!(
            recovered.node("work").unwrap().state,
            ExecutionNodeState::RecoveryRequired
        );
        assert_eq!(
            recovered.node("decide").unwrap().state,
            ExecutionNodeState::Pending
        );
        assert_eq!(recovered.snapshot().transitions.len(), 2);
    }

    #[test]
    fn recovery_never_retries_unknown_apply_but_exposes_finally_cleanup() {
        let mut graph = ExecutionGraph::new("mutation").unwrap();
        for (id, kind) in [
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
        graph.add_edge(edge("apply", "finalize")).unwrap();
        graph.add_edge(edge("release", "finalize")).unwrap();
        graph.start_node("apply").unwrap();

        let (recovered, report) =
            ExecutionGraph::recover_from_checkpoint(graph.checkpoint()).unwrap();

        assert_eq!(report.recovery_required_nodes, vec!["apply"]);
        assert_eq!(report.ready_nodes, vec!["release"]);
        assert_eq!(
            recovered.node("apply").unwrap().state,
            ExecutionNodeState::RecoveryRequired
        );
        assert_eq!(
            recovered.node("release").unwrap().state,
            ExecutionNodeState::Pending
        );
        assert_eq!(
            recovered.node("finalize").unwrap().state,
            ExecutionNodeState::Pending
        );
        assert_eq!(
            recovered
                .snapshot()
                .transitions
                .iter()
                .filter(|transition| {
                    transition.node_id == "apply" && transition.to == ExecutionNodeState::Running
                })
                .count(),
            1,
            "recovery records unknown effect and never starts Apply again"
        );
    }

    #[test]
    fn evidence_backed_reconciliation_resolves_apply_without_retrying_it() {
        let mut graph = ExecutionGraph::new("mutation").unwrap();
        for (id, kind) in [
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
        graph.add_edge(edge("apply", "finalize")).unwrap();
        graph.add_edge(edge("release", "finalize")).unwrap();
        graph.start_node("apply").unwrap();
        let (mut recovered, _) =
            ExecutionGraph::recover_from_checkpoint(graph.checkpoint()).unwrap();

        recovered
            .reconcile_node(
                "apply",
                ExecutionReconciliationDecision::ConfirmedSucceeded,
                "workspace content matches the reviewed patch hash",
                vec!["workspace:sha256:abc".into()],
                vec![ExecutionArtifact {
                    id: "artifact-apply-reconciled".into(),
                    producer_node: "apply".into(),
                    kind: "applied_patch".into(),
                    summary: "reconciled applied patch".into(),
                    evidence_refs: vec!["workspace:sha256:abc".into()],
                    uncertainties: Vec::new(),
                }],
            )
            .unwrap();

        assert_eq!(
            recovered.node("apply").unwrap().state,
            ExecutionNodeState::Succeeded
        );
        assert_eq!(recovered.ready_node_ids(), vec!["release"]);
        assert_eq!(recovered.snapshot().reconciliations.len(), 1);
        assert_eq!(
            recovered
                .snapshot()
                .transitions
                .iter()
                .filter(|transition| {
                    transition.node_id == "apply" && transition.to == ExecutionNodeState::Running
                })
                .count(),
            1,
            "reconciliation confirms external fact without retrying Apply"
        );

        let checkpoint =
            serde_json::from_str(&serde_json::to_string(&recovered.checkpoint()).unwrap()).unwrap();
        let (validated, report) = ExecutionGraph::recover_from_checkpoint(checkpoint).unwrap();
        assert!(report.recovery_required_nodes.is_empty());
        assert_eq!(validated.snapshot(), recovered.snapshot());
    }

    #[test]
    fn reconciliation_requires_evidence_and_preserves_unknown_state_on_rejection() {
        let mut graph = ExecutionGraph::new("mutation").unwrap();
        graph
            .add_node(ExecutionNode::pending(
                "apply",
                ExecutionNodeKind::Apply,
                "apply",
            ))
            .unwrap();
        graph.start_node("apply").unwrap();
        let (mut recovered, _) =
            ExecutionGraph::recover_from_checkpoint(graph.checkpoint()).unwrap();

        let error = recovered
            .reconcile_node(
                "apply",
                ExecutionReconciliationDecision::ConfirmedSucceeded,
                "looks successful",
                Vec::new(),
                Vec::new(),
            )
            .unwrap_err();

        assert_eq!(error, ExecutionGraphError::MissingReconciliationEvidence);
        assert_eq!(
            recovered.node("apply").unwrap().state,
            ExecutionNodeState::RecoveryRequired
        );
        assert!(recovered.snapshot().reconciliations.is_empty());
    }

    #[test]
    fn recovery_rejects_resolution_transition_without_reconciliation_record() {
        let mut graph = ExecutionGraph::new("mutation").unwrap();
        graph
            .add_node(ExecutionNode::pending(
                "apply",
                ExecutionNodeKind::Apply,
                "apply",
            ))
            .unwrap();
        graph.start_node("apply").unwrap();
        let (mut recovered, _) =
            ExecutionGraph::recover_from_checkpoint(graph.checkpoint()).unwrap();
        recovered
            .reconcile_node(
                "apply",
                ExecutionReconciliationDecision::ConfirmedFailed,
                "workspace does not contain the expected content",
                vec!["workspace:sha256:def".into()],
                Vec::new(),
            )
            .unwrap();
        let mut checkpoint = recovered.checkpoint();
        checkpoint.graph.reconciliations.clear();

        let error = ExecutionGraph::recover_from_checkpoint(checkpoint).unwrap_err();

        assert!(matches!(error, ExecutionGraphError::InvalidSnapshot(_)));
    }

    #[test]
    fn recovery_rejects_snapshot_state_that_does_not_match_history() {
        let mut graph = ExecutionGraph::new("task").unwrap();
        graph
            .add_node(ExecutionNode::pending(
                "work",
                ExecutionNodeKind::Work,
                "work",
            ))
            .unwrap();
        graph.start_node("work").unwrap();
        let mut checkpoint = graph.checkpoint();
        checkpoint.graph.nodes[0].state = ExecutionNodeState::Succeeded;

        let error = ExecutionGraph::recover_from_checkpoint(checkpoint).unwrap_err();

        assert!(matches!(error, ExecutionGraphError::InvalidSnapshot(_)));
    }
}
