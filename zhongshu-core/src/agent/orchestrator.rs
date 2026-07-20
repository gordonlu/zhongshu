use std::path::PathBuf;

use crate::agent::llm::{ChatCompletionRequest, Message};
use crate::agent::llm_registry::{LlmClient, LlmRegistry};
use crate::agent::profile::AgentProfile;
use crate::agent::report::Report;
use crate::agent::runtime::AgentRuntime;
use crate::agent::worker::Worker;
use crate::agent::AttentionLevel;
use crate::harness::architecture::index::ProjectIndex;
use crate::harness::trace::event::HarnessEvent;
use crate::integration::{
    DeeplosslessFileClaimOutcome, DeeplosslessFileReleaseOutcome, DeeplosslessProxy,
};
use crate::patch::{
    PatchAttemptFailure, PatchEngine, PatchOperation, PatchOperationKind, PatchResult,
};
use async_trait::async_trait;

/// A file-scoped sub-task assignment for a single worker.
#[derive(Debug, Clone)]
pub struct WorkerAssignment {
    pub worker_name: String,
    pub task_description: String,
    pub owned_files: Vec<PathBuf>,
    pub profile: AgentProfile,
}

/// A file edit conflict between two workers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub file: PathBuf,
    pub workers: Vec<String>,
}

/// A worker wrote a file outside its assigned ownership set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipViolation {
    pub worker: String,
    pub file: PathBuf,
    pub owned_files: Vec<PathBuf>,
    pub reason: String,
}

/// A static overlap in worker ownership before any worker starts editing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentFileOverlap {
    pub file: PathBuf,
    pub workers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerFileClaim {
    pub worker: String,
    pub file: PathBuf,
    pub operation: String,
    pub conv_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerFileClaimConflict {
    pub worker: String,
    pub file: PathBuf,
    pub holder: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerFileClaimReleaseFailure {
    pub worker: String,
    pub file: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerFileClaimReport {
    pub active_claims: Vec<WorkerFileClaim>,
    pub local_overlaps: Vec<AssignmentFileOverlap>,
    pub conflicts: Vec<WorkerFileClaimConflict>,
    pub release_failures: Vec<WorkerFileClaimReleaseFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerExecutionStatus {
    Completed,
    /// Workers returned results, but at least one result lacks fresh
    /// verification evidence and still needs parent verification.
    Submitted,
    BlockedBeforeExecution,
    WorkerFailed,
    CompletedWithReviewFindings,
}

#[derive(Debug, Clone)]
pub struct WorkerExecutionReport {
    pub status: WorkerExecutionStatus,
    pub reports: Vec<Report>,
    pub claim_report: WorkerFileClaimReport,
    pub conflicts: Vec<Conflict>,
    pub ownership_violations: Vec<OwnershipViolation>,
    pub release_failures: Vec<WorkerFileClaimReleaseFailure>,
    pub execution_error: Option<String>,
    pub trace_events: Vec<HarnessEvent>,
}

/// Result of the first production-oriented Lead → analyst → verifier handoff.
/// The analyst may submit an unverified report; final acceptance requires the
/// verifier to produce fresh successful verification evidence.
#[derive(Debug, Clone)]
pub struct LeadReviewReport {
    pub status: WorkerExecutionStatus,
    pub analyst: Report,
    pub verifier: Report,
    pub acceptance_reasons: Vec<String>,
    pub trace_events: Vec<HarnessEvent>,
}

impl WorkerExecutionReport {
    pub fn has_blockers(&self) -> bool {
        self.status != WorkerExecutionStatus::Completed
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerPatchProposal {
    pub worker: String,
    pub files: Vec<PathBuf>,
    pub summary: String,
    pub verification_commands: Vec<String>,
    pub operations: Vec<PatchOperation>,
}

impl WorkerPatchProposal {
    pub fn new(worker: impl Into<String>, files: Vec<PathBuf>, summary: impl Into<String>) -> Self {
        Self {
            worker: worker.into(),
            files,
            summary: summary.into(),
            verification_commands: Vec::new(),
            operations: Vec::new(),
        }
    }

    pub fn with_verification_commands(mut self, commands: Vec<String>) -> Self {
        self.verification_commands = commands;
        self
    }

    pub fn with_operations(mut self, operations: Vec<PatchOperation>) -> Self {
        self.operations = operations;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerMergeStatus {
    Approved,
    RequiresParentReview,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerPatchDecision {
    pub proposal: WorkerPatchProposal,
    pub approved: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerMergeReview {
    pub status: WorkerMergeStatus,
    pub decisions: Vec<WorkerPatchDecision>,
    pub blockers: Vec<String>,
}

impl WorkerMergeReview {
    pub fn has_blockers(&self) -> bool {
        self.status == WorkerMergeStatus::Blocked || !self.blockers.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct WorkerPatchApplyReport {
    pub applied: Vec<PatchResult>,
    pub failures: Vec<WorkerPatchApplyFailure>,
}

impl WorkerPatchApplyReport {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct WorkerPatchApplyFailure {
    pub worker: String,
    pub operation: PatchOperationKind,
    pub path: Option<PathBuf>,
    pub message: String,
    pub evidence: Option<crate::patch::PatchFailureEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerPatchPipelineStatus {
    Applied,
    Blocked,
    ApplyFailed,
}

#[derive(Debug, Clone)]
pub struct WorkerPatchPipelineReport {
    pub status: WorkerPatchPipelineStatus,
    pub execution: WorkerExecutionReport,
    pub merge_review: WorkerMergeReview,
    pub apply_report: WorkerPatchApplyReport,
}

impl WorkerPatchPipelineReport {
    pub fn passed(&self) -> bool {
        self.status == WorkerPatchPipelineStatus::Applied && self.apply_report.passed()
    }
}

#[async_trait]
pub trait FileClaimCoordinator: Send + Sync {
    async fn claim_file(
        &self,
        agent_id: &str,
        file_path: &str,
        operation: &str,
        conv_id: i64,
    ) -> anyhow::Result<DeeplosslessFileClaimOutcome>;

    async fn release_file(
        &self,
        agent_id: &str,
        file_path: &str,
    ) -> anyhow::Result<DeeplosslessFileReleaseOutcome>;
}

#[async_trait]
impl FileClaimCoordinator for DeeplosslessProxy {
    async fn claim_file(
        &self,
        agent_id: &str,
        file_path: &str,
        operation: &str,
        conv_id: i64,
    ) -> anyhow::Result<DeeplosslessFileClaimOutcome> {
        DeeplosslessProxy::claim_file(self, agent_id, file_path, operation, conv_id).await
    }

    async fn release_file(
        &self,
        agent_id: &str,
        file_path: &str,
    ) -> anyhow::Result<DeeplosslessFileReleaseOutcome> {
        DeeplosslessProxy::release_file(self, agent_id, file_path).await
    }
}

/// Parent orchestrator: splits work, launches workers, detects conflicts, parent-review.
///
/// NOTE: This module is implemented and tested, but NOT yet wired into any
/// production execution path. It is ready for integration when multi-worker
/// task splitting is needed. Currently, all agent tasks run through
/// `run_agent` / `run_agent_with_context` (single-worker) or
/// `Worker::execute` directly.
pub struct Orchestrator {
    pub runtime: AgentRuntime,
    pub registry: LlmRegistry,
    pub max_concurrent_workers: usize,
}

impl Orchestrator {
    pub fn new(runtime: AgentRuntime, registry: LlmRegistry) -> Self {
        Orchestrator {
            runtime,
            registry,
            max_concurrent_workers: 2,
        }
    }

    /// Split a high-level task into file-scoped worker assignments.
    pub fn split_task(
        &self,
        task_description: &str,
        profiles: &[AgentProfile],
        index: &ProjectIndex,
    ) -> Vec<WorkerAssignment> {
        if profiles.is_empty() {
            return Vec::new();
        }

        let files: Vec<&PathBuf> = index.files.keys().collect();
        if files.is_empty() {
            return vec![WorkerAssignment {
                worker_name: profiles[0].name.clone(),
                task_description: task_description.to_string(),
                owned_files: Vec::new(),
                profile: profiles[0].clone(),
            }];
        }

        let mut assignments: Vec<WorkerAssignment> = profiles
            .iter()
            .enumerate()
            .map(|(i, p)| WorkerAssignment {
                worker_name: p.name.clone(),
                task_description: format!("{task_description}\n\n负责的文件：第 {i} 组"),
                owned_files: Vec::new(),
                profile: p.clone(),
            })
            .collect();

        for (i, file) in files.iter().enumerate() {
            let idx = i % assignments.len();
            assignments[idx].owned_files.push((*file).clone());
        }

        for a in &mut assignments {
            if !a.owned_files.is_empty() {
                let file_list: Vec<String> = a
                    .owned_files
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect();
                a.task_description = format!(
                    "{}\n\n你负责的文件:\n{}",
                    task_description,
                    file_list.join("\n")
                );
            }
        }

        assignments
    }

    /// Run all worker assignments sequentially.
    pub async fn execute(&self, assignments: Vec<WorkerAssignment>) -> anyhow::Result<Vec<Report>> {
        self.ensure_worker_limit(assignments.len())?;
        let mut reports = Vec::new();

        for a in &assignments {
            reports.push(self.execute_assignment(a).await?);
        }

        Ok(reports)
    }

    /// Execute a bounded two-worker review handoff used by the desktop
    /// production entrypoint. The second worker receives the first worker's
    /// report, so this is an actual collaboration chain rather than two
    /// independent parallel prompts. Acceptance remains deterministic: an
    /// analyst report plus a freshly verified verifier result.
    pub async fn execute_review_handoff(
        &self,
        goal: &str,
        analyst_profile: AgentProfile,
        verifier_profile: AgentProfile,
        session_id: &str,
    ) -> anyhow::Result<LeadReviewReport> {
        self.ensure_worker_limit(2)?;
        let session_id = Some(session_id.to_string());
        let analyst_assignment = WorkerAssignment {
            worker_name: analyst_profile.name.clone(),
            task_description: format!(
                "作为分析员工审查以下任务，只收集事实、定位风险并提交报告，不修改文件：\n\n{goal}"
            ),
            owned_files: Vec::new(),
            profile: analyst_profile,
        };
        let mut trace_events = vec![worker_started_event(
            &analyst_assignment,
            session_id.clone(),
        )];
        let analyst = self.execute_assignment(&analyst_assignment).await?;
        trace_events.push(worker_completed_event(
            &analyst_assignment,
            session_id.clone(),
            analyst.success,
            report_status(&analyst),
            analyst.trace_events.len(),
        ));

        let verifier_assignment = WorkerAssignment {
            worker_name: verifier_profile.name.clone(),
            task_description: format!(
                "作为验证员工，独立复核分析报告并运行与任务直接相关的验证。不得修改文件；若无法获得新鲜验证证据，必须明确说明未验证。\n\n原始任务：\n{goal}\n\n分析员工报告：\n{}",
                analyst.findings
            ),
            owned_files: Vec::new(),
            profile: verifier_profile,
        };
        trace_events.push(worker_started_event(
            &verifier_assignment,
            session_id.clone(),
        ));
        let verifier = self.execute_assignment(&verifier_assignment).await?;
        trace_events.push(worker_completed_event(
            &verifier_assignment,
            session_id,
            verifier.success,
            report_status(&verifier),
            verifier.trace_events.len(),
        ));

        let analyst_submitted = matches!(
            analyst.outcome,
            crate::agent::RunOutcome::CompletedVerified
                | crate::agent::RunOutcome::CompletedUnverified
        );
        let verifier_verified = verifier.outcome == crate::agent::RunOutcome::CompletedVerified;
        let mut acceptance_reasons = Vec::new();
        if !analyst_submitted {
            acceptance_reasons.push(format!(
                "analysis worker ended with {}",
                report_status(&analyst)
            ));
        }
        if !verifier_verified {
            acceptance_reasons
                .push("verification worker did not produce fresh passing evidence".into());
            if verifier.outcome == crate::agent::RunOutcome::Blocked {
                acceptance_reasons.push(
                    "verification worker stopped at its completion gate; Lead kept the review pending"
                        .into(),
                );
            }
        }
        let status = if !analyst_submitted
            || matches!(
                verifier.outcome,
                crate::agent::RunOutcome::Failed
                    | crate::agent::RunOutcome::BudgetExhausted
                    | crate::agent::RunOutcome::Interrupted
            ) {
            WorkerExecutionStatus::WorkerFailed
        } else if verifier_verified {
            WorkerExecutionStatus::Completed
        } else {
            WorkerExecutionStatus::Submitted
        };

        Ok(LeadReviewReport {
            status,
            analyst,
            verifier,
            acceptance_reasons,
            trace_events,
        })
    }

    /// Execute file-owned worker assignments under deeplossless file claims.
    ///
    /// This composes the production safety sequence for worker orchestration:
    /// local overlap check, remote file claim, worker execution, parent-side
    /// conflict/ownership review, and best-effort release. Worker execution
    /// errors are returned inside the report so claim release failures are not
    /// lost.
    pub async fn execute_with_file_claims<C: FileClaimCoordinator>(
        &self,
        assignments: Vec<WorkerAssignment>,
        coordinator: &C,
        conv_id: i64,
        operation: &str,
    ) -> anyhow::Result<WorkerExecutionReport> {
        self.execute_with_file_claims_mode(
            assignments,
            coordinator,
            conv_id,
            operation,
            false,
            None,
        )
        .await
    }

    /// Execute file-owned worker assignments concurrently under the same
    /// claim/review/release contract as `execute_with_file_claims`.
    ///
    /// This is intended for independent read-only collection or review workers.
    /// File claims are still acquired before any worker starts so parent-side
    /// ownership remains explicit.
    pub async fn execute_with_file_claims_concurrent<C: FileClaimCoordinator>(
        &self,
        assignments: Vec<WorkerAssignment>,
        coordinator: &C,
        conv_id: i64,
        operation: &str,
    ) -> anyhow::Result<WorkerExecutionReport> {
        self.execute_with_file_claims_mode(assignments, coordinator, conv_id, operation, true, None)
            .await
    }

    /// Execute workers for a coding session and tag parent-side worker trace
    /// events with the session id.
    pub async fn execute_session_workers_with_file_claims<C: FileClaimCoordinator>(
        &self,
        session_id: impl Into<String>,
        assignments: Vec<WorkerAssignment>,
        coordinator: &C,
        conv_id: i64,
        operation: &str,
        concurrent: bool,
    ) -> anyhow::Result<WorkerExecutionReport> {
        let session_id = session_id.into();
        self.execute_with_file_claims_mode(
            assignments,
            coordinator,
            conv_id,
            operation,
            concurrent,
            Some(session_id.as_str()),
        )
        .await
    }

    async fn execute_with_file_claims_mode<C: FileClaimCoordinator>(
        &self,
        assignments: Vec<WorkerAssignment>,
        coordinator: &C,
        conv_id: i64,
        operation: &str,
        concurrent: bool,
        session_id: Option<&str>,
    ) -> anyhow::Result<WorkerExecutionReport> {
        self.ensure_worker_limit(assignments.len())?;
        let claim_report = self
            .claim_worker_files(&assignments, coordinator, conv_id, operation)
            .await?;
        if !claim_report.local_overlaps.is_empty() || !claim_report.conflicts.is_empty() {
            return Ok(WorkerExecutionReport {
                status: WorkerExecutionStatus::BlockedBeforeExecution,
                reports: Vec::new(),
                release_failures: claim_report.release_failures.clone(),
                claim_report,
                conflicts: Vec::new(),
                ownership_violations: Vec::new(),
                execution_error: None,
                trace_events: Vec::new(),
            });
        }

        let (reports, execution_error, trace_events) = self
            .execute_claimed_assignments(&assignments, concurrent, session_id)
            .await;

        let conflicts = self.detect_conflicts(&reports);
        let ownership_violations = self.detect_ownership_violations(&assignments, &reports);
        let mut release_failures = claim_report.release_failures.clone();
        release_failures.extend(
            self.release_worker_file_claims(&claim_report.active_claims, coordinator)
                .await,
        );

        let has_hard_failure = reports.iter().any(|report| {
            !matches!(
                report.outcome,
                crate::agent::RunOutcome::CompletedVerified
                    | crate::agent::RunOutcome::CompletedUnverified
            )
        });
        let has_unverified = reports
            .iter()
            .any(|report| report.outcome == crate::agent::RunOutcome::CompletedUnverified);
        let status = if execution_error.is_some() || has_hard_failure {
            WorkerExecutionStatus::WorkerFailed
        } else if !conflicts.is_empty()
            || !ownership_violations.is_empty()
            || !release_failures.is_empty()
        {
            WorkerExecutionStatus::CompletedWithReviewFindings
        } else if has_unverified {
            WorkerExecutionStatus::Submitted
        } else {
            WorkerExecutionStatus::Completed
        };

        Ok(WorkerExecutionReport {
            status,
            reports,
            claim_report,
            conflicts,
            ownership_violations,
            release_failures,
            execution_error,
            trace_events,
        })
    }

    fn ensure_worker_limit(&self, requested: usize) -> anyhow::Result<()> {
        if requested > self.max_concurrent_workers {
            anyhow::bail!(
                "worker limit exceeded: requested {requested}, maximum is {}",
                self.max_concurrent_workers
            );
        }
        Ok(())
    }

    async fn execute_claimed_assignments(
        &self,
        assignments: &[WorkerAssignment],
        concurrent: bool,
        session_id: Option<&str>,
    ) -> (Vec<Report>, Option<String>, Vec<HarnessEvent>) {
        let session_id = session_id.map(str::to_string);
        if concurrent {
            let mut trace_events = assignments
                .iter()
                .map(|assignment| worker_started_event(assignment, session_id.clone()))
                .collect::<Vec<_>>();
            let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(
                self.max_concurrent_workers.max(1),
            ));
            let results = futures::future::join_all(assignments.iter().map(|assignment| {
                let sem = sem.clone();
                async move {
                    let _permit = sem.acquire().await.expect("semaphore closed");
                    self.execute_assignment(assignment).await
                }
            }))
            .await;
            let mut reports = Vec::new();
            let mut execution_error = None;
            for (assignment, result) in assignments.iter().zip(results) {
                match result {
                    Ok(report) => {
                        trace_events.push(worker_completed_event(
                            assignment,
                            session_id.clone(),
                            report.success,
                            report_status(&report),
                            report.trace_events.len(),
                        ));
                        reports.push(report);
                    }
                    Err(error) if execution_error.is_none() => {
                        trace_events.push(worker_completed_event(
                            assignment,
                            session_id.clone(),
                            false,
                            "failed",
                            0,
                        ));
                        execution_error = Some(format!(
                            "worker '{}' failed: {error}",
                            assignment.worker_name
                        ));
                    }
                    Err(_) => trace_events.push(worker_completed_event(
                        assignment,
                        session_id.clone(),
                        false,
                        "failed",
                        0,
                    )),
                }
            }
            return (reports, execution_error, trace_events);
        }

        let mut reports = Vec::new();
        let mut trace_events = Vec::new();
        for assignment in assignments {
            trace_events.push(worker_started_event(assignment, session_id.clone()));
            match self.execute_assignment(assignment).await {
                Ok(report) => {
                    trace_events.push(worker_completed_event(
                        assignment,
                        session_id.clone(),
                        report.success,
                        report_status(&report),
                        report.trace_events.len(),
                    ));
                    reports.push(report);
                }
                Err(error) => {
                    trace_events.push(worker_completed_event(
                        assignment,
                        session_id.clone(),
                        false,
                        "failed",
                        0,
                    ));
                    return (
                        reports,
                        Some(format!(
                            "worker '{}' failed: {error}",
                            assignment.worker_name
                        )),
                        trace_events,
                    );
                }
            }
        }
        (reports, None, trace_events)
    }

    async fn execute_assignment(&self, assignment: &WorkerAssignment) -> anyhow::Result<Report> {
        let task = crate::task::Task {
            id: worker_task_id(assignment),
            source: "orchestrator".into(),
            tool: "agent".into(),
            arguments: serde_json::json!({
                "task": assignment.task_description,
            }),
        };

        Worker::execute(&self.runtime, &assignment.profile, task, None).await
    }

    /// Detect file edit conflicts across worker reports.
    pub fn detect_conflicts(&self, reports: &[Report]) -> Vec<Conflict> {
        let mut file_map: std::collections::BTreeMap<PathBuf, Vec<String>> =
            std::collections::BTreeMap::new();

        for report in reports {
            for event in &report.trace_events {
                if let HarnessEvent::FileEdit { path, .. } = event {
                    file_map
                        .entry(path.clone())
                        .or_default()
                        .push(report.worker.clone());
                }
            }
        }

        for workers in file_map.values_mut() {
            workers.sort();
            workers.dedup();
        }

        file_map
            .into_iter()
            .filter(|(_, workers)| workers.len() > 1)
            .map(|(file, workers)| Conflict { file, workers })
            .collect()
    }

    /// Detect writes outside each worker's assigned file ownership.
    ///
    /// Empty `owned_files` means ownership enforcement is disabled for that
    /// worker. This preserves the existing fallback path for repositories that
    /// have not been indexed yet.
    pub fn detect_ownership_violations(
        &self,
        assignments: &[WorkerAssignment],
        reports: &[Report],
    ) -> Vec<OwnershipViolation> {
        let ownership: std::collections::BTreeMap<String, Vec<PathBuf>> = assignments
            .iter()
            .map(|assignment| {
                (
                    assignment.worker_name.clone(),
                    normalize_owned_files(&assignment.owned_files),
                )
            })
            .collect();

        let mut violations = Vec::new();
        for report in reports {
            let owned_files = ownership.get(&report.worker);
            for event in &report.trace_events {
                if let HarnessEvent::FileEdit { path, .. } = event {
                    let file = normalize_path(path);
                    match owned_files {
                        Some(owned) if owned.is_empty() => {}
                        Some(owned)
                            if owned.iter().any(|owned| path_matches_owned(&file, owned)) => {}
                        Some(owned) => violations.push(OwnershipViolation {
                            worker: report.worker.clone(),
                            file,
                            owned_files: owned.clone(),
                            reason: "file is outside worker ownership".into(),
                        }),
                        None => violations.push(OwnershipViolation {
                            worker: report.worker.clone(),
                            file,
                            owned_files: Vec::new(),
                            reason: "worker has no assignment".into(),
                        }),
                    }
                }
            }
        }

        violations
    }

    /// Review worker patch proposals before the parent applies or merges them.
    ///
    /// This is intentionally a policy boundary only: workers can propose files
    /// and verification commands, but the parent still owns actual patch
    /// application through `PatchEngine`.
    pub fn review_worker_patch_proposals(
        &self,
        assignments: &[WorkerAssignment],
        execution: &WorkerExecutionReport,
        proposals: Vec<WorkerPatchProposal>,
    ) -> WorkerMergeReview {
        let mut blockers = execution_blockers(execution);
        let ownership: std::collections::BTreeMap<String, Vec<PathBuf>> = assignments
            .iter()
            .map(|assignment| {
                (
                    assignment.worker_name.clone(),
                    normalize_owned_files(&assignment.owned_files),
                )
            })
            .collect();

        let mut decisions = Vec::new();
        for mut proposal in proposals {
            proposal.files = normalize_owned_files(&proposal.files);
            let mut reasons = Vec::new();
            if proposal.summary.trim().is_empty() {
                reasons.push("proposal summary is empty".into());
            }
            if proposal.files.is_empty() {
                reasons.push("proposal does not name any files".into());
            }
            if proposal.operations.is_empty() {
                reasons.push("proposal does not include patch operations".into());
            }
            for operation in &proposal.operations {
                let operation_path = normalize_path(&operation.path().to_path_buf());
                if !proposal
                    .files
                    .iter()
                    .any(|file| path_matches_owned(&operation_path, file))
                {
                    reasons.push(format!(
                        "operation path {} is not listed in proposal files",
                        operation_path.display()
                    ));
                }
            }
            match ownership.get(&proposal.worker) {
                Some(owned_files) if owned_files.is_empty() => {
                    reasons.push("worker has unscoped ownership; parent review required".into());
                }
                Some(owned_files) => {
                    for file in &proposal.files {
                        if !owned_files
                            .iter()
                            .any(|owned| path_matches_owned(file, owned))
                        {
                            reasons.push(format!(
                                "file {} is outside worker ownership",
                                file.display()
                            ));
                        }
                    }
                }
                None => reasons.push("worker has no assignment".into()),
            }

            let approved = reasons.is_empty();
            decisions.push(WorkerPatchDecision {
                proposal,
                approved,
                reasons,
            });
        }

        if decisions.is_empty() {
            blockers.push("no worker patch proposals submitted".into());
        }

        let status = if execution.status == WorkerExecutionStatus::WorkerFailed
            || execution.status == WorkerExecutionStatus::BlockedBeforeExecution
            || !execution.conflicts.is_empty()
            || !execution.claim_report.local_overlaps.is_empty()
            || !execution.claim_report.conflicts.is_empty()
        {
            WorkerMergeStatus::Blocked
        } else if !blockers.is_empty() || decisions.iter().any(|decision| !decision.approved) {
            WorkerMergeStatus::RequiresParentReview
        } else {
            WorkerMergeStatus::Approved
        };

        WorkerMergeReview {
            status,
            decisions,
            blockers,
        }
    }

    /// Apply approved worker patch proposals through `PatchEngine`.
    ///
    /// This method deliberately refuses non-approved reviews. It performs the
    /// parent-owned application step, while PatchEngine enforces read-before-
    /// write, stale reads, path safety, encoding, and line-ending behavior.
    pub fn apply_worker_patch_review(
        &self,
        engine: &mut PatchEngine,
        review: &WorkerMergeReview,
        session_id: Option<String>,
        trace_events: &mut Vec<HarnessEvent>,
    ) -> WorkerPatchApplyReport {
        if review.status != WorkerMergeStatus::Approved
            || review.decisions.iter().any(|decision| !decision.approved)
        {
            return WorkerPatchApplyReport {
                applied: Vec::new(),
                failures: vec![WorkerPatchApplyFailure {
                    worker: "orchestrator".into(),
                    operation: PatchOperationKind::Read,
                    path: None,
                    message: "worker merge review is not approved".into(),
                    evidence: None,
                }],
            };
        }

        let mut applied = Vec::new();
        let mut failures = Vec::new();
        for decision in &review.decisions {
            for operation in &decision.proposal.operations {
                if operation.kind() != PatchOperationKind::CreateFile {
                    if let Err(error) = engine.read(operation.path()) {
                        failures.push(WorkerPatchApplyFailure {
                            worker: decision.proposal.worker.clone(),
                            operation: PatchOperationKind::Read,
                            path: Some(operation.path().to_path_buf()),
                            message: error.to_string(),
                            evidence: Some(crate::patch::PatchFailureEvidence::from_error(
                                PatchOperationKind::Read,
                                Some(operation.path().to_path_buf()),
                                &error,
                            )),
                        });
                        continue;
                    }
                }

                match engine.apply_operation(operation.clone()) {
                    Ok(result) => {
                        trace_events.push(patch_preview_event(
                            session_id.clone(),
                            operation.path(),
                            operation.kind_name(),
                            &result,
                        ));
                        trace_events.push(patch_applied_event(
                            session_id.clone(),
                            operation.path(),
                            operation.kind_name(),
                            true,
                        ));
                        applied.push(result);
                    }
                    Err(failure) => {
                        failures.push(apply_failure_from_patch(&decision.proposal.worker, failure))
                    }
                }
            }
        }

        WorkerPatchApplyReport { applied, failures }
    }

    /// Execute workers, review their patch proposals, and apply approved
    /// operations through `PatchEngine`.
    ///
    /// This is the production-safe orchestration entrypoint for worker-owned
    /// patches: execution must be clean, merge review must approve every
    /// proposal, and actual file writes go through PatchEngine.
    pub async fn execute_worker_patch_pipeline<C: FileClaimCoordinator>(
        &self,
        assignments: Vec<WorkerAssignment>,
        coordinator: &C,
        conv_id: i64,
        operation: &str,
        concurrent: bool,
        proposals: Vec<WorkerPatchProposal>,
        engine: &mut PatchEngine,
    ) -> anyhow::Result<WorkerPatchPipelineReport> {
        self.execute_worker_patch_pipeline_mode(
            None,
            assignments,
            coordinator,
            conv_id,
            operation,
            concurrent,
            proposals,
            engine,
        )
        .await
    }

    pub async fn execute_session_worker_patch_pipeline<C: FileClaimCoordinator>(
        &self,
        session_id: impl Into<String>,
        assignments: Vec<WorkerAssignment>,
        coordinator: &C,
        conv_id: i64,
        operation: &str,
        concurrent: bool,
        proposals: Vec<WorkerPatchProposal>,
        engine: &mut PatchEngine,
    ) -> anyhow::Result<WorkerPatchPipelineReport> {
        self.execute_worker_patch_pipeline_mode(
            Some(session_id.into()),
            assignments,
            coordinator,
            conv_id,
            operation,
            concurrent,
            proposals,
            engine,
        )
        .await
    }

    async fn execute_worker_patch_pipeline_mode<C: FileClaimCoordinator>(
        &self,
        session_id: Option<String>,
        assignments: Vec<WorkerAssignment>,
        coordinator: &C,
        conv_id: i64,
        operation: &str,
        concurrent: bool,
        proposals: Vec<WorkerPatchProposal>,
        engine: &mut PatchEngine,
    ) -> anyhow::Result<WorkerPatchPipelineReport> {
        let mut execution = self
            .execute_with_file_claims_mode(
                assignments.clone(),
                coordinator,
                conv_id,
                operation,
                concurrent,
                session_id.as_deref(),
            )
            .await?;
        let merge_review = self.review_worker_patch_proposals(&assignments, &execution, proposals);
        let apply_report = if merge_review.status == WorkerMergeStatus::Approved {
            self.apply_worker_patch_review(
                engine,
                &merge_review,
                session_id.clone(),
                &mut execution.trace_events,
            )
        } else {
            WorkerPatchApplyReport {
                applied: Vec::new(),
                failures: Vec::new(),
            }
        };
        let status = if merge_review.status != WorkerMergeStatus::Approved {
            WorkerPatchPipelineStatus::Blocked
        } else if apply_report.passed() {
            WorkerPatchPipelineStatus::Applied
        } else {
            WorkerPatchPipelineStatus::ApplyFailed
        };

        Ok(WorkerPatchPipelineReport {
            status,
            execution,
            merge_review,
            apply_report,
        })
    }

    /// Detect duplicate or nested file ownership before workers start.
    ///
    /// deeplossless rejects conflicts across conversations. Within one
    /// conversation, Zhongshu still needs this local check so two workers do
    /// not claim overlapping ownership and race each other.
    pub fn detect_assignment_file_overlaps(
        &self,
        assignments: &[WorkerAssignment],
    ) -> Vec<AssignmentFileOverlap> {
        let mut overlaps: std::collections::BTreeMap<PathBuf, Vec<String>> =
            std::collections::BTreeMap::new();

        for (left_index, left) in assignments.iter().enumerate() {
            let left_files = normalize_owned_files(&left.owned_files);
            if left_files.is_empty() {
                continue;
            }

            for right in assignments.iter().skip(left_index + 1) {
                let right_files = normalize_owned_files(&right.owned_files);
                if right_files.is_empty() {
                    continue;
                }

                for left_file in &left_files {
                    for right_file in &right_files {
                        if paths_overlap(left_file, right_file) {
                            let file = overlap_key(left_file, right_file);
                            let workers = overlaps.entry(file).or_default();
                            workers.push(left.worker_name.clone());
                            workers.push(right.worker_name.clone());
                        }
                    }
                }
            }
        }

        overlaps
            .into_iter()
            .map(|(file, mut workers)| {
                workers.sort();
                workers.dedup();
                AssignmentFileOverlap { file, workers }
            })
            .collect()
    }

    /// Claim every assigned file before worker execution.
    ///
    /// On the first remote conflict, claims acquired during this call are
    /// released before returning so the caller never receives a partial active
    /// claim set.
    pub async fn claim_worker_files<C: FileClaimCoordinator>(
        &self,
        assignments: &[WorkerAssignment],
        coordinator: &C,
        conv_id: i64,
        operation: &str,
    ) -> anyhow::Result<WorkerFileClaimReport> {
        if conv_id <= 0 {
            return Err(anyhow::anyhow!("conv_id must be positive"));
        }
        if operation.trim().is_empty() {
            return Err(anyhow::anyhow!("operation must not be empty"));
        }

        let local_overlaps = self.detect_assignment_file_overlaps(assignments);
        if !local_overlaps.is_empty() {
            return Ok(WorkerFileClaimReport {
                local_overlaps,
                ..WorkerFileClaimReport::default()
            });
        }

        let mut active_claims = Vec::new();
        let mut conflicts = Vec::new();
        let mut release_failures = Vec::new();

        for assignment in assignments {
            for file in normalize_owned_files(&assignment.owned_files) {
                let file_path = file.to_string_lossy().to_string();
                match coordinator
                    .claim_file(&assignment.worker_name, &file_path, operation, conv_id)
                    .await?
                {
                    DeeplosslessFileClaimOutcome::Claimed { .. } => {
                        active_claims.push(WorkerFileClaim {
                            worker: assignment.worker_name.clone(),
                            file,
                            operation: operation.to_string(),
                            conv_id,
                        });
                    }
                    DeeplosslessFileClaimOutcome::Conflict { conflict } => {
                        conflicts.push(WorkerFileClaimConflict {
                            worker: assignment.worker_name.clone(),
                            file,
                            holder: conflict.agent_id,
                            message: conflict.message,
                        });
                        release_failures.extend(
                            self.release_worker_file_claims(&active_claims, coordinator)
                                .await,
                        );
                        active_claims.clear();
                        return Ok(WorkerFileClaimReport {
                            active_claims,
                            local_overlaps: Vec::new(),
                            conflicts,
                            release_failures,
                        });
                    }
                }
            }
        }

        Ok(WorkerFileClaimReport {
            active_claims,
            local_overlaps: Vec::new(),
            conflicts,
            release_failures,
        })
    }

    pub async fn release_worker_file_claims<C: FileClaimCoordinator>(
        &self,
        claims: &[WorkerFileClaim],
        coordinator: &C,
    ) -> Vec<WorkerFileClaimReleaseFailure> {
        let mut failures = Vec::new();
        for claim in claims {
            let file_path = claim.file.to_string_lossy().to_string();
            match coordinator.release_file(&claim.worker, &file_path).await {
                Ok(DeeplosslessFileReleaseOutcome::Released { .. }) => {}
                Ok(DeeplosslessFileReleaseOutcome::Missing { missing }) => {
                    failures.push(WorkerFileClaimReleaseFailure {
                        worker: claim.worker.clone(),
                        file: claim.file.clone(),
                        message: missing.message,
                    });
                }
                Err(error) => failures.push(WorkerFileClaimReleaseFailure {
                    worker: claim.worker.clone(),
                    file: claim.file.clone(),
                    message: error.to_string(),
                }),
            }
        }
        failures
    }

    /// Parent review: unify worker reports into a single coherent report.
    pub async fn parent_review(
        &self,
        task: &str,
        reports: &[Report],
        conflicts: &[Conflict],
        parent_client: &LlmClient,
    ) -> anyhow::Result<Report> {
        let mut worker_summaries = String::new();
        for (i, r) in reports.iter().enumerate() {
            worker_summaries.push_str(&format!(
                "\n--- Worker {} ({}) ---\n{}\n摘要: {}\n置信度: {:.2}",
                i + 1,
                r.worker,
                r.findings,
                r.summary,
                r.confidence,
            ));
        }

        let mut conflict_text = String::new();
        if conflicts.is_empty() {
            conflict_text = "无冲突".into();
        } else {
            for c in conflicts {
                conflict_text.push_str(&format!(
                    "\n- 文件 {} 被多个 worker 编辑: {}",
                    c.file.display(),
                    c.workers.join(", ")
                ));
            }
        }

        let prompt = format!(
            r#"你是一个代码审查协调员。多个 worker 已经完成了以下任务的子任务：

## 原始任务
{task}

## Worker 报告
{worker_summaries}

## 检测到的冲突
{conflict_text}

请整合以上报告，输出一个统一的工作摘要。要求：
1. 总结每个 worker 的发现和产出
2. 指出任何冲突及其处理建议
3. 给出整体置信度评估
4. 保持简洁，聚焦于实质性产出"#
        );

        let messages = vec![
            Message::system("你是一个专业的代码审查协调员，善于整合多个并行 worker 的报告。"),
            Message::user(prompt),
        ];

        let request = ChatCompletionRequest {
            model: parent_client.model.clone(),
            messages,
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: parent_client.temperature,
            max_tokens: None,
            reasoning_effort: None,
        };

        let response = parent_client
            .provider
            .chat(request)
            .await
            .map_err(|e| anyhow::anyhow!("parent review LLM call failed: {e}"))?;

        let content = response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        Ok(Report {
            task_id: "parent-review".into(),
            worker: "orchestrator".into(),
            summary: if content.chars().count() > 200 {
                format!("{}...", content.chars().take(200).collect::<String>())
            } else {
                content.clone()
            },
            findings: content,
            success: false,
            outcome: crate::agent::RunOutcome::CompletedUnverified,
            confidence: 0.7,
            attention: AttentionLevel::Digest,
            trace_events: Vec::new(),
        })
    }
}

fn normalize_owned_files(files: &[PathBuf]) -> Vec<PathBuf> {
    let mut normalized: Vec<PathBuf> = files.iter().map(|path| normalize_path(path)).collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

fn worker_started_event(assignment: &WorkerAssignment, session_id: Option<String>) -> HarnessEvent {
    HarnessEvent::WorkerStarted {
        session_id,
        worker: assignment.worker_name.clone(),
        task_id: worker_task_id(assignment),
        owned_files: normalize_owned_files(&assignment.owned_files),
    }
}

fn worker_completed_event(
    assignment: &WorkerAssignment,
    session_id: Option<String>,
    success: bool,
    status: &str,
    trace_event_count: usize,
) -> HarnessEvent {
    HarnessEvent::WorkerCompleted {
        session_id,
        worker: assignment.worker_name.clone(),
        task_id: worker_task_id(assignment),
        success,
        status: status.to_string(),
        trace_event_count,
    }
}

fn report_status(report: &Report) -> &'static str {
    match report.outcome {
        crate::agent::RunOutcome::CompletedVerified => "completed",
        crate::agent::RunOutcome::CompletedUnverified => "submitted",
        crate::agent::RunOutcome::Interrupted => "interrupted",
        crate::agent::RunOutcome::Blocked | crate::agent::RunOutcome::BudgetExhausted => "blocked",
        crate::agent::RunOutcome::Failed => "failed",
    }
}

fn patch_preview_event(
    session_id: Option<String>,
    path: &std::path::Path,
    operation: &str,
    diff_result: &PatchResult,
) -> HarnessEvent {
    HarnessEvent::PatchPreview {
        session_id,
        path: path.to_path_buf(),
        operation: operation.to_string(),
        diff_summary: format!(
            "+{} -{}",
            diff_result.diff.added_lines, diff_result.diff.removed_lines
        ),
        diff: Some(crate::patch::PatchDiffPayload::from_diff(
            &diff_result.diff,
            format!(
                "+{} -{}",
                diff_result.diff.added_lines, diff_result.diff.removed_lines
            ),
        )),
    }
}

fn patch_applied_event(
    session_id: Option<String>,
    path: &std::path::Path,
    operation: &str,
    changed: bool,
) -> HarnessEvent {
    HarnessEvent::PatchApplied {
        session_id,
        path: path.to_path_buf(),
        operation: operation.to_string(),
        changed,
    }
}

fn worker_task_id(assignment: &WorkerAssignment) -> String {
    format!("worker-{}", assignment.worker_name)
}

fn execution_blockers(execution: &WorkerExecutionReport) -> Vec<String> {
    let mut blockers = Vec::new();
    if execution.status == WorkerExecutionStatus::Submitted {
        blockers.push("worker results were submitted without fresh verification".into());
    }
    if let Some(error) = &execution.execution_error {
        blockers.push(error.clone());
    }
    for overlap in &execution.claim_report.local_overlaps {
        blockers.push(format!(
            "assignment overlap on {} between {}",
            overlap.file.display(),
            overlap.workers.join(", ")
        ));
    }
    for conflict in &execution.claim_report.conflicts {
        blockers.push(format!(
            "file claim conflict on {} held by {}: {}",
            conflict.file.display(),
            conflict.holder.as_deref().unwrap_or("unknown"),
            conflict.message
        ));
    }
    for conflict in &execution.conflicts {
        blockers.push(format!(
            "worker edit conflict on {} between {}",
            conflict.file.display(),
            conflict.workers.join(", ")
        ));
    }
    for violation in &execution.ownership_violations {
        blockers.push(format!(
            "worker {} edited {} outside ownership: {}",
            violation.worker,
            violation.file.display(),
            violation.reason
        ));
    }
    for failure in &execution.release_failures {
        blockers.push(format!(
            "failed to release claim for {} on {}: {}",
            failure.worker,
            failure.file.display(),
            failure.message
        ));
    }
    blockers
}

fn apply_failure_from_patch(worker: &str, failure: PatchAttemptFailure) -> WorkerPatchApplyFailure {
    WorkerPatchApplyFailure {
        worker: worker.to_string(),
        operation: failure.evidence.operation,
        path: failure.evidence.path.clone(),
        message: failure.error.to_string(),
        evidence: Some(failure.evidence),
    }
}

fn normalize_path(path: &PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn path_matches_owned(file: &PathBuf, owned: &PathBuf) -> bool {
    file == owned || file.starts_with(owned)
}

fn paths_overlap(left: &PathBuf, right: &PathBuf) -> bool {
    path_matches_owned(left, right) || path_matches_owned(right, left)
}

fn overlap_key(left: &PathBuf, right: &PathBuf) -> PathBuf {
    if left.components().count() <= right.components().count() {
        left.clone()
    } else {
        right.clone()
    }
}

#[cfg(test)]
#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::agent::llm::{ChatCompletionResponse, FinalChoice, LlmProvider};
    use crate::agent::AgentBudget;
    use crate::harness::architecture::index::FileIndex;
    use crate::tool::ToolRegistry;
    use async_trait::async_trait;
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;
    pub struct MockProvider;

    #[derive(Clone)]
    pub struct ConcurrentMockProvider {
        pub in_flight: Arc<AtomicUsize>,
        pub max_in_flight: Arc<AtomicUsize>,
    }
    pub struct MockFileClaimCoordinator {
        pub conflict_files: BTreeSet<String>,
        pub missing_releases: BTreeSet<String>,
        pub claimed: Mutex<Vec<(String, String)>>,
        pub released: Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        async fn chat(
            &self,
            _request: ChatCompletionRequest,
        ) -> anyhow::Result<ChatCompletionResponse> {
            Ok(ChatCompletionResponse {
                choices: vec![FinalChoice {
                    message: Message::assistant("统一审查结果：一切正常。"),
                    finish_reason: Some("stop".into()),
                }],
                usage: None,
            })
        }
        async fn stream_chat(
            &self,
            _request: ChatCompletionRequest,
            mut _on_event: Box<dyn FnMut(crate::agent::llm::StreamEvent) + Send>,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        fn model_name(&self) -> &str {
            "mock"
        }
        fn change_model(&self, _model: &str) -> Arc<dyn LlmProvider> {
            Arc::new(MockProvider)
        }
    }

    #[async_trait]
    impl LlmProvider for ConcurrentMockProvider {
        async fn chat(
            &self,
            _request: ChatCompletionRequest,
        ) -> anyhow::Result<ChatCompletionResponse> {
            let current = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_in_flight.fetch_max(current, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(ChatCompletionResponse {
                choices: vec![FinalChoice {
                    message: Message::assistant("worker done"),
                    finish_reason: Some("stop".into()),
                }],
                usage: None,
            })
        }

        async fn stream_chat(
            &self,
            _request: ChatCompletionRequest,
            mut _on_event: Box<dyn FnMut(crate::agent::llm::StreamEvent) + Send>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn model_name(&self) -> &str {
            "concurrent-mock"
        }

        fn change_model(&self, _model: &str) -> Arc<dyn LlmProvider> {
            Arc::new(self.clone())
        }
    }

    impl MockFileClaimCoordinator {
        pub fn new() -> Self {
            Self {
                conflict_files: BTreeSet::new(),
                missing_releases: BTreeSet::new(),
                claimed: Mutex::new(Vec::new()),
                released: Mutex::new(Vec::new()),
            }
        }

        pub fn with_conflict(mut self, file_path: &str) -> Self {
            self.conflict_files.insert(test_file_key(file_path));
            self
        }

        pub fn with_missing_release(mut self, file_path: &str) -> Self {
            self.missing_releases.insert(test_file_key(file_path));
            self
        }

        pub fn claimed(&self) -> Vec<(String, String)> {
            self.claimed.lock().unwrap().clone()
        }

        pub fn released(&self) -> Vec<(String, String)> {
            self.released.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl FileClaimCoordinator for MockFileClaimCoordinator {
        async fn claim_file(
            &self,
            agent_id: &str,
            file_path: &str,
            _operation: &str,
            conv_id: i64,
        ) -> anyhow::Result<DeeplosslessFileClaimOutcome> {
            let file_key = test_file_key(file_path);
            if self.conflict_files.contains(&file_key) {
                return Ok(DeeplosslessFileClaimOutcome::Conflict {
                    conflict: crate::integration::DeeplosslessFileClaimConflict {
                        file_path: file_key,
                        agent_id: Some("remote-worker".into()),
                        message: "remote conflict".into(),
                    },
                });
            }
            self.claimed
                .lock()
                .unwrap()
                .push((agent_id.to_string(), file_key.clone()));
            Ok(DeeplosslessFileClaimOutcome::Claimed {
                claim: crate::integration::DeeplosslessFileClaimResult {
                    status: "claimed".into(),
                    agent_id: agent_id.to_string(),
                    file_path: file_key,
                    conv_id,
                },
            })
        }

        async fn release_file(
            &self,
            agent_id: &str,
            file_path: &str,
        ) -> anyhow::Result<DeeplosslessFileReleaseOutcome> {
            let file_key = test_file_key(file_path);
            self.released
                .lock()
                .unwrap()
                .push((agent_id.to_string(), file_key.clone()));
            if self.missing_releases.contains(&file_key) {
                return Ok(DeeplosslessFileReleaseOutcome::Missing {
                    missing: crate::integration::DeeplosslessFileReleaseMissing {
                        agent_id: agent_id.to_string(),
                        file_path: file_key,
                        message: "missing claim".into(),
                    },
                });
            }
            Ok(DeeplosslessFileReleaseOutcome::Released {
                release: crate::integration::DeeplosslessFileReleaseResult {
                    status: "released".into(),
                    file_path: file_key,
                },
            })
        }
    }

    fn test_file_key(file_path: &str) -> String {
        file_path.replace('/', "\\")
    }

    fn dummy_profile(name: &str) -> AgentProfile {
        AgentProfile::new(
            name,
            "你是一个测试 worker。",
            vec![],
            AgentBudget::default(),
        )
    }

    pub fn dummy_runtime() -> AgentRuntime {
        AgentRuntime::new(
            MockProvider,
            ToolRegistry::new(),
            "mock-model",
            AgentBudget::default(),
        )
    }

    fn make_index(files: &[&str]) -> ProjectIndex {
        let mut index = ProjectIndex::new(PathBuf::from("."));
        for f in files {
            let path = PathBuf::from(f);
            index.files.insert(
                path.clone(),
                FileIndex {
                    path,
                    imports: vec![],
                    items: vec![],
                    parse_error: None,
                },
            );
        }
        index
    }

    fn completed_execution() -> WorkerExecutionReport {
        WorkerExecutionReport {
            status: WorkerExecutionStatus::Completed,
            reports: Vec::new(),
            claim_report: WorkerFileClaimReport::default(),
            conflicts: Vec::new(),
            ownership_violations: Vec::new(),
            release_failures: Vec::new(),
            execution_error: None,
            trace_events: Vec::new(),
        }
    }

    #[test]
    fn split_task_empty_profiles_returns_empty() {
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let index = make_index(&["a.rs"]);
        let result = orch.split_task("test", &[], &index);
        assert!(result.is_empty());
    }

    #[test]
    fn split_task_assigns_all_files() {
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let index = make_index(&["a.rs", "b.rs", "c.rs", "d.rs"]);
        let profiles = vec![dummy_profile("w1"), dummy_profile("w2")];
        let assignments = orch.split_task("refactor", &profiles, &index);

        assert_eq!(assignments.len(), 2);
        // Each worker should have roughly equal files
        let total: usize = assignments.iter().map(|a| a.owned_files.len()).sum();
        assert_eq!(total, 4);
        // All original files are assigned
        let all: std::collections::HashSet<&PathBuf> =
            assignments.iter().flat_map(|a| &a.owned_files).collect();
        assert!(all.contains(&PathBuf::from("a.rs")));
        assert!(all.contains(&PathBuf::from("b.rs")));
        assert!(all.contains(&PathBuf::from("c.rs")));
        assert!(all.contains(&PathBuf::from("d.rs")));
    }

    #[test]
    fn split_task_single_profile_gets_all() {
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let index = make_index(&["a.rs", "b.rs"]);
        let profiles = vec![dummy_profile("w1")];
        let assignments = orch.split_task("test", &profiles, &index);
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].owned_files.len(), 2);
    }

    #[test]
    fn split_task_empty_index_creates_fallback() {
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let index = ProjectIndex::new(PathBuf::from("."));
        let profiles = vec![dummy_profile("w1")];
        let assignments = orch.split_task("test", &profiles, &index);
        assert_eq!(assignments.len(), 1);
        assert!(assignments[0].owned_files.is_empty());
    }

    #[test]
    fn detect_conflicts_no_overlap() {
        let report_a = Report {
            task_id: "t1".into(),
            worker: "w1".into(),
            summary: "".into(),
            findings: "".into(),
            confidence: 0.5,
            success: true,
            outcome: crate::agent::RunOutcome::CompletedVerified,
            attention: AttentionLevel::Digest,
            trace_events: vec![HarnessEvent::FileEdit {
                path: PathBuf::from("a.rs"),
                diff_hash: "abc".into(),
                diff: None,
            }],
        };
        let report_b = Report {
            task_id: "t2".into(),
            worker: "w2".into(),
            summary: "".into(),
            findings: "".into(),
            confidence: 0.5,
            success: true,
            outcome: crate::agent::RunOutcome::CompletedVerified,
            attention: AttentionLevel::Digest,
            trace_events: vec![HarnessEvent::FileEdit {
                path: PathBuf::from("b.rs"),
                diff_hash: "def".into(),
                diff: None,
            }],
        };

        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let conflicts = orch.detect_conflicts(&[report_a, report_b]);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn detect_conflicts_detects_overlap() {
        let report_a = Report {
            task_id: "t1".into(),
            worker: "w1".into(),
            summary: "".into(),
            findings: "".into(),
            confidence: 0.5,
            success: true,
            outcome: crate::agent::RunOutcome::CompletedVerified,
            attention: AttentionLevel::Digest,
            trace_events: vec![HarnessEvent::FileEdit {
                path: PathBuf::from("shared.rs"),
                diff_hash: "abc".into(),
                diff: None,
            }],
        };
        let report_b = Report {
            task_id: "t2".into(),
            worker: "w2".into(),
            summary: "".into(),
            findings: "".into(),
            confidence: 0.5,
            success: true,
            outcome: crate::agent::RunOutcome::CompletedVerified,
            attention: AttentionLevel::Digest,
            trace_events: vec![HarnessEvent::FileEdit {
                path: PathBuf::from("shared.rs"),
                diff_hash: "def".into(),
                diff: None,
            }],
        };

        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let conflicts = orch.detect_conflicts(&[report_a, report_b]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].file, PathBuf::from("shared.rs"));
    }

    #[test]
    fn ownership_allows_edits_inside_owned_files() {
        let assignment = WorkerAssignment {
            worker_name: "w1".into(),
            task_description: "edit owned".into(),
            owned_files: vec![PathBuf::from("src/a.rs")],
            profile: dummy_profile("w1"),
        };
        let report = Report {
            task_id: "t1".into(),
            worker: "w1".into(),
            summary: "".into(),
            findings: "".into(),
            confidence: 0.5,
            success: true,
            outcome: crate::agent::RunOutcome::CompletedVerified,
            attention: AttentionLevel::Digest,
            trace_events: vec![HarnessEvent::FileEdit {
                path: PathBuf::from("src/a.rs"),
                diff_hash: "abc".into(),
                diff: None,
            }],
        };

        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let violations = orch.detect_ownership_violations(&[assignment], &[report]);

        assert!(violations.is_empty());
    }

    #[test]
    fn ownership_detects_edit_outside_owned_files() {
        let assignment = WorkerAssignment {
            worker_name: "w1".into(),
            task_description: "edit owned".into(),
            owned_files: vec![PathBuf::from("src/a.rs")],
            profile: dummy_profile("w1"),
        };
        let report = Report {
            task_id: "t1".into(),
            worker: "w1".into(),
            summary: "".into(),
            findings: "".into(),
            confidence: 0.5,
            success: true,
            outcome: crate::agent::RunOutcome::CompletedVerified,
            attention: AttentionLevel::Digest,
            trace_events: vec![HarnessEvent::FileEdit {
                path: PathBuf::from("src/b.rs"),
                diff_hash: "abc".into(),
                diff: None,
            }],
        };

        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let violations = orch.detect_ownership_violations(&[assignment], &[report]);

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].worker, "w1");
        assert_eq!(violations[0].file, PathBuf::from("src/b.rs"));
    }

    #[test]
    fn ownership_allows_unscoped_fallback_assignment() {
        let assignment = WorkerAssignment {
            worker_name: "w1".into(),
            task_description: "fallback".into(),
            owned_files: Vec::new(),
            profile: dummy_profile("w1"),
        };
        let report = Report {
            task_id: "t1".into(),
            worker: "w1".into(),
            summary: "".into(),
            findings: "".into(),
            confidence: 0.5,
            success: true,
            outcome: crate::agent::RunOutcome::CompletedVerified,
            attention: AttentionLevel::Digest,
            trace_events: vec![HarnessEvent::FileEdit {
                path: PathBuf::from("src/anything.rs"),
                diff_hash: "abc".into(),
                diff: None,
            }],
        };

        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let violations = orch.detect_ownership_violations(&[assignment], &[report]);

        assert!(violations.is_empty());
    }

    #[test]
    fn ownership_detects_unknown_worker_edits() {
        let report = Report {
            task_id: "t1".into(),
            worker: "unknown".into(),
            summary: "".into(),
            findings: "".into(),
            confidence: 0.5,
            success: true,
            outcome: crate::agent::RunOutcome::CompletedVerified,
            attention: AttentionLevel::Digest,
            trace_events: vec![HarnessEvent::FileEdit {
                path: PathBuf::from("src/a.rs"),
                diff_hash: "abc".into(),
                diff: None,
            }],
        };

        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let violations = orch.detect_ownership_violations(&[], &[report]);

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].reason, "worker has no assignment");
    }

    #[test]
    fn merge_review_approves_owned_patch_proposal() {
        let assignment = WorkerAssignment {
            worker_name: "w1".into(),
            task_description: "edit owned".into(),
            owned_files: vec![PathBuf::from("src/a.rs")],
            profile: dummy_profile("w1"),
        };
        let proposal =
            WorkerPatchProposal::new("w1", vec![PathBuf::from("src/a.rs")], "update owned file")
                .with_verification_commands(vec!["cargo test -p zhongshu-core".into()])
                .with_operations(vec![PatchOperation::Replace(
                    crate::patch::ReplaceRequest::once("src/a.rs", "old", "new"),
                )]);
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());

        let review = orch.review_worker_patch_proposals(
            &[assignment],
            &completed_execution(),
            vec![proposal],
        );

        assert_eq!(review.status, WorkerMergeStatus::Approved);
        assert!(review.blockers.is_empty());
        assert_eq!(review.decisions.len(), 1);
        assert!(review.decisions[0].approved);
    }

    #[test]
    fn merge_review_requires_parent_review_for_out_of_scope_patch() {
        let assignment = WorkerAssignment {
            worker_name: "w1".into(),
            task_description: "edit owned".into(),
            owned_files: vec![PathBuf::from("src/a.rs")],
            profile: dummy_profile("w1"),
        };
        let proposal =
            WorkerPatchProposal::new("w1", vec![PathBuf::from("src/b.rs")], "edit other file")
                .with_operations(vec![PatchOperation::Replace(
                    crate::patch::ReplaceRequest::once("src/b.rs", "old", "new"),
                )]);
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());

        let review = orch.review_worker_patch_proposals(
            &[assignment],
            &completed_execution(),
            vec![proposal],
        );

        assert_eq!(review.status, WorkerMergeStatus::RequiresParentReview);
        assert_eq!(review.decisions.len(), 1);
        assert!(!review.decisions[0].approved);
        assert!(review.decisions[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("outside worker ownership")));
    }

    #[test]
    fn merge_review_blocks_when_worker_execution_conflicted() {
        let assignment = WorkerAssignment {
            worker_name: "w1".into(),
            task_description: "edit owned".into(),
            owned_files: vec![PathBuf::from("src/a.rs")],
            profile: dummy_profile("w1"),
        };
        let mut execution = completed_execution();
        execution.status = WorkerExecutionStatus::CompletedWithReviewFindings;
        execution.conflicts = vec![Conflict {
            file: PathBuf::from("src/a.rs"),
            workers: vec!["w1".into(), "w2".into()],
        }];
        let proposal =
            WorkerPatchProposal::new("w1", vec![PathBuf::from("src/a.rs")], "edit owned file")
                .with_operations(vec![PatchOperation::Replace(
                    crate::patch::ReplaceRequest::once("src/a.rs", "old", "new"),
                )]);
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());

        let review = orch.review_worker_patch_proposals(&[assignment], &execution, vec![proposal]);

        assert_eq!(review.status, WorkerMergeStatus::Blocked);
        assert!(review.has_blockers());
        assert!(review
            .blockers
            .iter()
            .any(|blocker| blocker.contains("worker edit conflict")));
    }

    #[test]
    fn merge_review_requires_parent_review_without_patch_operations() {
        let assignment = WorkerAssignment {
            worker_name: "w1".into(),
            task_description: "edit owned".into(),
            owned_files: vec![PathBuf::from("src/a.rs")],
            profile: dummy_profile("w1"),
        };
        let proposal =
            WorkerPatchProposal::new("w1", vec![PathBuf::from("src/a.rs")], "summary only");
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());

        let review = orch.review_worker_patch_proposals(
            &[assignment],
            &completed_execution(),
            vec![proposal],
        );

        assert_eq!(review.status, WorkerMergeStatus::RequiresParentReview);
        assert!(!review.decisions[0].approved);
        assert!(review.decisions[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("patch operations")));
    }

    #[test]
    fn apply_worker_patch_review_applies_approved_operations() {
        let temp = tempfile::tempdir().expect("tempdir");
        let src = temp.path().join("src");
        std::fs::create_dir(&src).expect("create src");
        std::fs::write(src.join("a.rs"), "old\n").expect("write file");
        let mut engine = PatchEngine::new(temp.path()).expect("patch engine");
        let assignment = WorkerAssignment {
            worker_name: "w1".into(),
            task_description: "edit owned".into(),
            owned_files: vec![PathBuf::from("src/a.rs")],
            profile: dummy_profile("w1"),
        };
        let proposal =
            WorkerPatchProposal::new("w1", vec![PathBuf::from("src/a.rs")], "update owned file")
                .with_operations(vec![PatchOperation::Replace(
                    crate::patch::ReplaceRequest::once("src/a.rs", "old", "new"),
                )]);
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let review = orch.review_worker_patch_proposals(
            &[assignment],
            &completed_execution(),
            vec![proposal],
        );

        let report = orch.apply_worker_patch_review(&mut engine, &review, None, &mut vec![]);

        assert!(report.passed());
        assert_eq!(report.applied.len(), 1);
        assert_eq!(
            std::fs::read_to_string(src.join("a.rs")).expect("read file"),
            "new\n"
        );
    }

    #[test]
    fn apply_worker_patch_review_refuses_unapproved_review() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("a.rs"), "old\n").expect("write file");
        let mut engine = PatchEngine::new(temp.path()).expect("patch engine");
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let review = WorkerMergeReview {
            status: WorkerMergeStatus::RequiresParentReview,
            decisions: Vec::new(),
            blockers: Vec::new(),
        };

        let report = orch.apply_worker_patch_review(&mut engine, &review, None, &mut vec![]);

        assert!(!report.passed());
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures[0].message.contains("not approved"));
        assert_eq!(
            std::fs::read_to_string(temp.path().join("a.rs")).expect("read file"),
            "old\n"
        );
    }

    #[tokio::test]
    async fn worker_patch_pipeline_blocks_unverified_worker_patch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let src = temp.path().join("src");
        std::fs::create_dir(&src).expect("create src");
        std::fs::write(src.join("a.rs"), "old\n").expect("write file");
        let mut engine = PatchEngine::new(temp.path()).expect("patch engine");
        let assignments = vec![WorkerAssignment {
            worker_name: "w1".into(),
            task_description: "edit owned".into(),
            owned_files: vec![PathBuf::from("src/a.rs")],
            profile: dummy_profile("w1"),
        }];
        let proposals = vec![WorkerPatchProposal::new(
            "w1",
            vec![PathBuf::from("src/a.rs")],
            "update owned file",
        )
        .with_operations(vec![PatchOperation::Replace(
            crate::patch::ReplaceRequest::once("src/a.rs", "old", "new"),
        )])];
        let coordinator = MockFileClaimCoordinator::new();
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());

        let report = orch
            .execute_worker_patch_pipeline(
                assignments,
                &coordinator,
                7,
                "edit",
                false,
                proposals,
                &mut engine,
            )
            .await
            .expect("worker patch pipeline");

        assert_eq!(report.status, WorkerPatchPipelineStatus::Blocked);
        assert!(!report.passed());
        assert!(report.apply_report.applied.is_empty());
        assert_eq!(report.execution.status, WorkerExecutionStatus::Submitted);
        assert_eq!(
            std::fs::read_to_string(src.join("a.rs")).expect("read file"),
            "old\n"
        );
        assert_eq!(
            coordinator.released(),
            vec![("w1".into(), "src\\a.rs".into())]
        );
    }

    #[tokio::test]
    async fn worker_patch_pipeline_blocks_unapproved_patch_without_writing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let src = temp.path().join("src");
        std::fs::create_dir(&src).expect("create src");
        std::fs::write(src.join("a.rs"), "old\n").expect("write file");
        let mut engine = PatchEngine::new(temp.path()).expect("patch engine");
        let assignments = vec![WorkerAssignment {
            worker_name: "w1".into(),
            task_description: "edit owned".into(),
            owned_files: vec![PathBuf::from("src/a.rs")],
            profile: dummy_profile("w1"),
        }];
        let proposals = vec![WorkerPatchProposal::new(
            "w1",
            vec![PathBuf::from("src/b.rs")],
            "edit other file",
        )
        .with_operations(vec![PatchOperation::Replace(
            crate::patch::ReplaceRequest::once("src/b.rs", "old", "new"),
        )])];
        let coordinator = MockFileClaimCoordinator::new();
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());

        let report = orch
            .execute_worker_patch_pipeline(
                assignments,
                &coordinator,
                7,
                "edit",
                false,
                proposals,
                &mut engine,
            )
            .await
            .expect("worker patch pipeline");

        assert_eq!(report.status, WorkerPatchPipelineStatus::Blocked);
        assert_eq!(
            report.merge_review.status,
            WorkerMergeStatus::RequiresParentReview
        );
        assert!(report.apply_report.applied.is_empty());
        assert_eq!(
            std::fs::read_to_string(src.join("a.rs")).expect("read file"),
            "old\n"
        );
    }

    #[test]
    fn detects_assignment_file_overlaps_before_remote_claims() {
        let assignments = vec![
            WorkerAssignment {
                worker_name: "w1".into(),
                task_description: "edit".into(),
                owned_files: vec![PathBuf::from("src")],
                profile: dummy_profile("w1"),
            },
            WorkerAssignment {
                worker_name: "w2".into(),
                task_description: "edit".into(),
                owned_files: vec![PathBuf::from("src/a.rs")],
                profile: dummy_profile("w2"),
            },
        ];

        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let overlaps = orch.detect_assignment_file_overlaps(&assignments);

        assert_eq!(overlaps.len(), 1);
        assert_eq!(overlaps[0].file, PathBuf::from("src"));
        assert_eq!(overlaps[0].workers, vec!["w1", "w2"]);
    }

    #[tokio::test]
    async fn claim_worker_files_claims_each_owned_file() {
        let assignments = vec![
            WorkerAssignment {
                worker_name: "w1".into(),
                task_description: "edit".into(),
                owned_files: vec![PathBuf::from("src/a.rs")],
                profile: dummy_profile("w1"),
            },
            WorkerAssignment {
                worker_name: "w2".into(),
                task_description: "edit".into(),
                owned_files: vec![PathBuf::from("src/b.rs")],
                profile: dummy_profile("w2"),
            },
        ];
        let coordinator = MockFileClaimCoordinator::new();
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());

        let report = orch
            .claim_worker_files(&assignments, &coordinator, 7, "edit")
            .await
            .expect("claim worker files");

        assert_eq!(report.active_claims.len(), 2);
        assert!(report.conflicts.is_empty());
        assert!(report.local_overlaps.is_empty());
        assert_eq!(
            coordinator.claimed(),
            vec![
                ("w1".into(), "src\\a.rs".into()),
                ("w2".into(), "src\\b.rs".into())
            ]
        );
    }

    #[tokio::test]
    async fn claim_worker_files_releases_acquired_claims_on_conflict() {
        let assignments = vec![
            WorkerAssignment {
                worker_name: "w1".into(),
                task_description: "edit".into(),
                owned_files: vec![PathBuf::from("src/a.rs")],
                profile: dummy_profile("w1"),
            },
            WorkerAssignment {
                worker_name: "w2".into(),
                task_description: "edit".into(),
                owned_files: vec![PathBuf::from("src/b.rs")],
                profile: dummy_profile("w2"),
            },
        ];
        let coordinator = MockFileClaimCoordinator::new().with_conflict("src\\b.rs");
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());

        let report = orch
            .claim_worker_files(&assignments, &coordinator, 7, "edit")
            .await
            .expect("claim worker files");

        assert!(report.active_claims.is_empty());
        assert_eq!(report.conflicts.len(), 1);
        assert_eq!(report.conflicts[0].holder.as_deref(), Some("remote-worker"));
        assert_eq!(
            coordinator.released(),
            vec![("w1".into(), "src\\a.rs".into())]
        );
    }

    #[tokio::test]
    async fn release_worker_file_claims_reports_missing_claims() {
        let coordinator = MockFileClaimCoordinator::new().with_missing_release("src\\a.rs");
        let claims = vec![WorkerFileClaim {
            worker: "w1".into(),
            file: PathBuf::from("src/a.rs"),
            operation: "edit".into(),
            conv_id: 7,
        }];
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());

        let failures = orch.release_worker_file_claims(&claims, &coordinator).await;

        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].message, "missing claim");
    }

    #[tokio::test]
    async fn execute_with_file_claims_runs_workers_and_releases_claims() {
        let assignments = vec![WorkerAssignment {
            worker_name: "w1".into(),
            task_description: "inspect".into(),
            owned_files: vec![PathBuf::from("src/a.rs")],
            profile: dummy_profile("w1"),
        }];
        let coordinator = MockFileClaimCoordinator::new();
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());

        let report = orch
            .execute_with_file_claims(assignments, &coordinator, 7, "read")
            .await
            .expect("execute with file claims");

        assert_eq!(report.status, WorkerExecutionStatus::Submitted);
        assert!(report.has_blockers());
        assert_eq!(report.reports.len(), 1);
        assert_eq!(report.claim_report.active_claims.len(), 1);
        assert_eq!(
            coordinator.released(),
            vec![("w1".into(), "src\\a.rs".into())]
        );
    }

    #[tokio::test]
    async fn execute_with_file_claims_concurrent_runs_workers_in_parallel() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_in_flight = Arc::new(AtomicUsize::new(0));
        let runtime = AgentRuntime::new(
            ConcurrentMockProvider {
                in_flight: in_flight.clone(),
                max_in_flight: max_in_flight.clone(),
            },
            ToolRegistry::new(),
            "mock-model",
            AgentBudget::default(),
        );
        let assignments = vec![
            WorkerAssignment {
                worker_name: "w1".into(),
                task_description: "inspect a".into(),
                owned_files: vec![PathBuf::from("src/a.rs")],
                profile: dummy_profile("w1"),
            },
            WorkerAssignment {
                worker_name: "w2".into(),
                task_description: "inspect b".into(),
                owned_files: vec![PathBuf::from("src/b.rs")],
                profile: dummy_profile("w2"),
            },
        ];
        let coordinator = MockFileClaimCoordinator::new();
        let orch = Orchestrator::new(runtime, LlmRegistry::new());

        let report = orch
            .execute_with_file_claims_concurrent(assignments, &coordinator, 7, "read")
            .await
            .expect("execute with file claims");

        assert_eq!(report.status, WorkerExecutionStatus::Submitted);
        assert_eq!(report.reports.len(), 2);
        assert_eq!(report.trace_events.len(), 4);
        assert!(
            max_in_flight.load(Ordering::SeqCst) >= 2,
            "workers should overlap in-flight execution"
        );
        assert_eq!(
            coordinator.released(),
            vec![
                ("w1".into(), "src\\a.rs".into()),
                ("w2".into(), "src\\b.rs".into())
            ]
        );
    }

    #[tokio::test]
    async fn execute_rejects_more_than_configured_worker_limit() {
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let profile = dummy_profile("worker");
        let assignments = (0..3)
            .map(|index| WorkerAssignment {
                worker_name: format!("worker-{index}"),
                task_description: "test".into(),
                owned_files: Vec::new(),
                profile: profile.clone(),
            })
            .collect();

        let error = orch.execute(assignments).await.unwrap_err();
        assert!(error.to_string().contains("worker limit exceeded"));
    }

    #[tokio::test]
    async fn review_handoff_keeps_unverified_workers_submitted() {
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());

        let report = orch
            .execute_review_handoff(
                "review current changes",
                dummy_profile("analyst"),
                dummy_profile("verifier"),
                "session-review",
            )
            .await
            .expect("review handoff");

        assert_eq!(
            report.status,
            WorkerExecutionStatus::Submitted,
            "analyst={:?}, verifier={:?}, reasons={:?}",
            report.analyst.outcome,
            report.verifier.outcome,
            report.acceptance_reasons
        );
        assert_eq!(report.trace_events.len(), 4);
        assert!(report
            .acceptance_reasons
            .iter()
            .any(|reason| reason.contains("fresh passing evidence")));
        assert!(matches!(
            report.trace_events.first(),
            Some(HarnessEvent::WorkerStarted { session_id, worker, .. })
                if session_id.as_deref() == Some("session-review") && worker == "analyst"
        ));
        assert!(matches!(
            report.trace_events.get(2),
            Some(HarnessEvent::WorkerStarted { session_id, worker, .. })
                if session_id.as_deref() == Some("session-review") && worker == "verifier"
        ));
    }

    #[tokio::test]
    async fn execute_session_workers_with_file_claims_tags_worker_trace() {
        let assignments = vec![WorkerAssignment {
            worker_name: "w1".into(),
            task_description: "inspect".into(),
            owned_files: vec![PathBuf::from("src/a.rs")],
            profile: dummy_profile("w1"),
        }];
        let coordinator = MockFileClaimCoordinator::new();
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());

        let report = orch
            .execute_session_workers_with_file_claims(
                "session-1",
                assignments,
                &coordinator,
                7,
                "read",
                false,
            )
            .await
            .expect("execute session workers");

        assert_eq!(report.status, WorkerExecutionStatus::Submitted);
        assert!(matches!(
            report.trace_events.first(),
            Some(HarnessEvent::WorkerStarted {
                session_id,
                worker,
                task_id,
                owned_files,
            }) if session_id.as_deref() == Some("session-1")
                && worker == "w1"
                && task_id == "worker-w1"
                && owned_files == &vec![PathBuf::from("src/a.rs")]
        ));
        assert!(matches!(
            report.trace_events.get(1),
            Some(HarnessEvent::WorkerCompleted {
                session_id,
                worker,
                task_id,
                success: false,
                status,
                trace_event_count,
            }) if session_id.as_deref() == Some("session-1")
                && worker == "w1"
                && task_id == "worker-w1"
                && status == "submitted"
                && *trace_event_count > 0
        ));
    }

    #[tokio::test]
    async fn execute_with_file_claims_blocks_on_remote_conflict_without_reports() {
        let assignments = vec![WorkerAssignment {
            worker_name: "w1".into(),
            task_description: "edit".into(),
            owned_files: vec![PathBuf::from("src/a.rs")],
            profile: dummy_profile("w1"),
        }];
        let coordinator = MockFileClaimCoordinator::new().with_conflict("src\\a.rs");
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());

        let report = orch
            .execute_with_file_claims(assignments, &coordinator, 7, "edit")
            .await
            .expect("execute with file claims");

        assert_eq!(report.status, WorkerExecutionStatus::BlockedBeforeExecution);
        assert!(report.has_blockers());
        assert!(report.reports.is_empty());
        assert_eq!(report.claim_report.conflicts.len(), 1);
        assert!(coordinator.released().is_empty());
    }

    #[tokio::test]
    async fn execute_with_file_claims_reports_release_failure() {
        let assignments = vec![WorkerAssignment {
            worker_name: "w1".into(),
            task_description: "inspect".into(),
            owned_files: vec![PathBuf::from("src/a.rs")],
            profile: dummy_profile("w1"),
        }];
        let coordinator = MockFileClaimCoordinator::new().with_missing_release("src\\a.rs");
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());

        let report = orch
            .execute_with_file_claims(assignments, &coordinator, 7, "read")
            .await
            .expect("execute with file claims");

        assert_eq!(
            report.status,
            WorkerExecutionStatus::CompletedWithReviewFindings
        );
        assert!(report.has_blockers());
        assert_eq!(report.reports.len(), 1);
        assert_eq!(report.release_failures.len(), 1);
        assert_eq!(report.release_failures[0].message, "missing claim");
    }

    #[tokio::test]
    async fn parent_review_uses_mock_provider() {
        let client = LlmClient {
            provider: Arc::new(MockProvider),
            model: "mock".into(),
            profile_name: "test".into(),
            reasoning_effort: None,
            temperature: None,
            max_context_tokens: None,
        };

        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let report = orch
            .parent_review("添加 login 功能", &[], &[], &client)
            .await
            .expect("parent review should succeed");

        assert!(report.findings.contains("一切正常"));
        assert_eq!(report.worker, "orchestrator");
    }
}
