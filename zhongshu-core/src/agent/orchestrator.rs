use std::path::PathBuf;

use crate::agent::llm::{ChatCompletionRequest, Message};
use crate::agent::llm_registry::{LlmClient, LlmRegistry};
use crate::agent::organization::{
    AssignmentAuthority, CollaborationMode, DispatchTarget, EmployeeAssignment, OrganizationRouter,
    OrganizationTaskRequest, StaffingDecision, StaffingMode, StaffingPolicy, StaffingRequest,
    DEFAULT_MAX_EMPLOYEE_ROSTER, DEFAULT_MAX_WORKERS_PER_TASK,
};
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

#[derive(Debug, Clone)]
pub struct StaffedTask {
    pub decision: StaffingDecision,
    /// Empty for direct tasks and blocked staffing decisions. Partial staffing
    /// is never exposed as executable work.
    pub assignments: Vec<WorkerAssignment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationExecutionStatus {
    AwaitingManager,
    Blocked,
    Completed,
    Submitted,
    WorkerFailed,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmployeeWorkReport {
    pub assignment: EmployeeAssignment,
    pub reports_to: AssignmentAuthority,
    pub report: Report,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OrganizationExecutionReport {
    pub task_id: String,
    pub assigned_by: AssignmentAuthority,
    pub target: DispatchTarget,
    pub collaboration: CollaborationMode,
    pub status: OrganizationExecutionStatus,
    pub staffing: StaffingDecision,
    pub employee_reports: Vec<EmployeeWorkReport>,
    pub execution_error: Option<String>,
    pub trace_events: Vec<HarnessEvent>,
}

impl OrganizationExecutionReport {
    pub fn can_finalize(&self) -> bool {
        self.status == OrganizationExecutionStatus::Completed
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OrganizationFileScope {
    pub employee: String,
    pub owned_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagerAcceptanceStatus {
    AwaitingManager,
    Blocked,
    Accepted,
    ApplyFailed,
    AppliedWithReleaseFailures,
}

#[derive(Debug, Clone)]
pub struct ManagerAcceptanceReport {
    pub manager: String,
    pub status: ManagerAcceptanceStatus,
    pub summary: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct OrganizationMutationReport {
    pub task_id: String,
    pub staffing: StaffingDecision,
    pub employee_reports: Vec<EmployeeWorkReport>,
    pub pipeline: Option<WorkerPatchPipelineReport>,
    pub manager_acceptance: ManagerAcceptanceReport,
}

impl OrganizationMutationReport {
    pub fn can_finalize(&self) -> bool {
        self.manager_acceptance.status == ManagerAcceptanceStatus::Accepted
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// Result of the dedicated Lead → analyst → verifier review pipeline.
/// The analyst may submit an unverified report; final acceptance requires the
/// verifier to produce fresh successful verification evidence.
#[derive(Debug, Clone)]
pub struct LeadReviewReport {
    pub status: WorkerExecutionStatus,
    pub recovery: ReviewPipelineRecovery,
    pub analyst: Report,
    pub verifier: Report,
    pub acceptance_reasons: Vec<String>,
    pub trace_events: Vec<HarnessEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewPipelineRecovery {
    NotNeeded,
    Succeeded,
    Failed,
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

pub const SUBMIT_PATCH_PROPOSAL_TOOL: &str = "submit_patch_proposal";

/// Exact payload accepted from a worker. Unknown fields are rejected and the
/// worker identity is supplied by the orchestrator, never trusted from model
/// output.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchProposalSubmission {
    pub summary: String,
    pub files: Vec<PathBuf>,
    #[serde(default)]
    pub verification_commands: Vec<String>,
    pub operations: Vec<PatchOperation>,
}

#[derive(Debug, Default)]
struct PatchProposalCollector {
    proposals: std::collections::BTreeMap<String, WorkerPatchProposal>,
    errors: Vec<String>,
}

impl PatchProposalCollector {
    fn record(&mut self, worker: &str, submission: PatchProposalSubmission) -> Result<(), String> {
        let semantic_error = if submission.summary.trim().is_empty() {
            Some("summary must not be empty".to_string())
        } else if submission.files.is_empty()
            || submission
                .files
                .iter()
                .any(|path| path.as_os_str().is_empty())
        {
            Some("files must contain at least one non-empty path".to_string())
        } else if submission.operations.is_empty()
            || submission
                .operations
                .iter()
                .any(|operation| operation.path().as_os_str().is_empty())
        {
            Some("operations must contain at least one operation with a non-empty path".to_string())
        } else if submission
            .verification_commands
            .iter()
            .any(|command| command.trim().is_empty())
        {
            Some("verification_commands must not contain empty commands".to_string())
        } else {
            None
        };
        if let Some(reason) = semantic_error {
            let error = format!("employee '{worker}' submitted invalid patch proposal: {reason}");
            self.errors.push(error.clone());
            return Err(error);
        }
        if self.proposals.contains_key(worker) {
            let error = format!("employee '{worker}' submitted more than one patch proposal");
            self.errors.push(error.clone());
            return Err(error);
        }
        self.proposals.insert(
            worker.to_string(),
            WorkerPatchProposal {
                worker: worker.to_string(),
                files: submission.files,
                summary: submission.summary,
                verification_commands: submission.verification_commands,
                operations: submission.operations,
            },
        );
        Ok(())
    }
}

fn validate_patch_submission_shape(arguments: &serde_json::Value) -> Result<(), String> {
    let operations = arguments
        .as_object()
        .and_then(|object| object.get("operations"))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "operations must be an array".to_string())?;
    for (index, operation) in operations.iter().enumerate() {
        let object = operation
            .as_object()
            .ok_or_else(|| format!("operations[{index}] must be an object"))?;
        let operation_type = object
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("operations[{index}].type must be a string"))?;
        let allowed: &[&str] = match operation_type {
            "replace" => &["type", "path", "old_text", "new_text", "replace_all"],
            "multi_replace" => &["type", "path", "edits"],
            "write_file" => &[
                "type",
                "path",
                "content",
                "allow_create",
                "encoding",
                "line_ending",
            ],
            other => return Err(format!("operations[{index}] has unknown type '{other}'")),
        };
        if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
            return Err(format!(
                "operations[{index}] contains unknown field '{key}'"
            ));
        }
        if operation_type == "multi_replace" {
            let edits = object
                .get("edits")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| format!("operations[{index}].edits must be an array"))?;
            for (edit_index, edit) in edits.iter().enumerate() {
                let edit = edit.as_object().ok_or_else(|| {
                    format!("operations[{index}].edits[{edit_index}] must be an object")
                })?;
                let allowed_edit = ["old_text", "new_text", "replace_all"];
                if let Some(key) = edit
                    .keys()
                    .find(|key| !allowed_edit.contains(&key.as_str()))
                {
                    return Err(format!(
                        "operations[{index}].edits[{edit_index}] contains unknown field '{key}'"
                    ));
                }
            }
        }
    }
    Ok(())
}

struct SubmitPatchProposalTool {
    worker: String,
    collector: std::sync::Arc<std::sync::Mutex<PatchProposalCollector>>,
}

#[async_trait]
impl crate::tool::Tool for SubmitPatchProposalTool {
    fn name(&self) -> &str {
        SUBMIT_PATCH_PROPOSAL_TOOL
    }

    fn description(&self) -> &str {
        "Submit exactly one structured patch proposal for deterministic manager review. This tool records a proposal but does not modify files."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["summary", "files", "operations"],
            "properties": {
                "summary": { "type": "string", "minLength": 1 },
                "files": {
                    "type": "array",
                    "minItems": 1,
                    "items": { "type": "string", "minLength": 1 }
                },
                "verification_commands": {
                    "type": "array",
                    "items": { "type": "string", "minLength": 1 }
                },
                "operations": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "oneOf": [
                            {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["type", "path", "old_text", "new_text", "replace_all"],
                                "properties": {
                                    "type": { "const": "replace" },
                                    "path": { "type": "string", "minLength": 1 },
                                    "old_text": { "type": "string" },
                                    "new_text": { "type": "string" },
                                    "replace_all": { "type": "boolean" }
                                }
                            },
                            {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["type", "path", "edits"],
                                "properties": {
                                    "type": { "const": "multi_replace" },
                                    "path": { "type": "string", "minLength": 1 },
                                    "edits": {
                                        "type": "array",
                                        "minItems": 1,
                                        "items": {
                                            "type": "object",
                                            "additionalProperties": false,
                                            "required": ["old_text", "new_text", "replace_all"],
                                            "properties": {
                                                "old_text": { "type": "string" },
                                                "new_text": { "type": "string" },
                                                "replace_all": { "type": "boolean" }
                                            }
                                        }
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["type", "path", "content", "allow_create"],
                                "properties": {
                                    "type": { "const": "write_file" },
                                    "path": { "type": "string", "minLength": 1 },
                                    "content": { "type": "string" },
                                    "allow_create": { "type": "boolean" },
                                    "encoding": {
                                        "type": ["string", "null"],
                                        "enum": ["utf8", "utf8_bom", "utf16_le", null]
                                    },
                                    "line_ending": {
                                        "type": ["string", "null"],
                                        "enum": ["lf", "crlf", "mixed", "none", null]
                                    }
                                }
                            }
                        ]
                    }
                }
            }
        })
    }

    fn spec(&self) -> crate::tool::ToolSpec {
        crate::tool::ToolSpec::new(SUBMIT_PATCH_PROPOSAL_TOOL)
            .with_effect(crate::tool::ToolEffect::Memory)
            .read_only(true)
            .workspace_scope(crate::tool::WorkspaceScope::WorkspaceOnly)
            .requires_approval(false)
            .side_effect(crate::tool::SideEffect::ReadOnly)
    }

    async fn execute(&self, arguments: &serde_json::Value) -> crate::tool::ToolOutput {
        if let Err(error) = validate_patch_submission_shape(arguments) {
            self.collector.lock().unwrap().errors.push(format!(
                "employee '{}' submitted invalid patch proposal: {error}",
                self.worker
            ));
            return crate::tool::ToolOutput::error(format!(
                "invalid patch proposal schema: {error}"
            ));
        }
        let submission = match serde_json::from_value::<PatchProposalSubmission>(arguments.clone())
        {
            Ok(submission) => submission,
            Err(error) => {
                let message = format!("invalid patch proposal schema: {error}");
                self.collector.lock().unwrap().errors.push(format!(
                    "employee '{}' submitted invalid patch proposal: {error}",
                    self.worker
                ));
                return crate::tool::ToolOutput::error(message);
            }
        };
        match self
            .collector
            .lock()
            .unwrap()
            .record(&self.worker, submission)
        {
            Ok(()) => crate::tool::ToolOutput::success(serde_json::json!({
                "accepted_for_review": true,
                "employee": self.worker
            })),
            Err(error) => crate::tool::ToolOutput::error(error),
        }
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
    AppliedWithReviewFindings,
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
/// The dedicated review pipeline is wired into the desktop application. The
/// general organization executor has separate read-only and mutation paths.
/// Mutation work requires explicit ownership, file claims, structured patch
/// proposals, deterministic parent review, and parent-owned patch application.
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
            max_concurrent_workers: DEFAULT_MAX_WORKERS_PER_TASK,
        }
    }

    /// Apply the deterministic organization contract to a Lead-produced
    /// staffing request and turn a valid decision into executable assignments.
    pub fn staff_task(&self, request: &StaffingRequest, roster: &[AgentProfile]) -> StaffedTask {
        let router = self.organization_router();
        let decision = router.route(request, roster);
        self.build_staffed_task(request, roster, decision)
    }

    pub fn staff_organization_task(
        &self,
        request: &OrganizationTaskRequest,
        roster: &[AgentProfile],
    ) -> StaffedTask {
        let router = self.organization_router();
        let decision = match &request.target {
            DispatchTarget::ManagerSelected => router.route(&request.staffing, roster),
            DispatchTarget::Employee { employee } => {
                router.route_to_employee(&request.staffing, roster, employee)
            }
        };
        self.build_staffed_task(&request.staffing, roster, decision)
    }

    fn organization_router(&self) -> OrganizationRouter {
        OrganizationRouter::new(StaffingPolicy {
            max_workers_per_task: self.max_concurrent_workers,
            max_employee_roster: DEFAULT_MAX_EMPLOYEE_ROSTER,
        })
    }

    fn build_staffed_task(
        &self,
        request: &StaffingRequest,
        roster: &[AgentProfile],
        mut decision: StaffingDecision,
    ) -> StaffedTask {
        if !decision.can_execute() || decision.assignments.is_empty() {
            return StaffedTask {
                decision,
                assignments: Vec::new(),
            };
        }

        let assignments: Option<Vec<WorkerAssignment>> = decision
            .assignments
            .iter()
            .map(|selected| {
                roster
                    .iter()
                    .find(|profile| profile.name == selected.employee)
                    .map(|profile| WorkerAssignment {
                        worker_name: profile.name.clone(),
                        task_description: format!(
                            "组织目标：{}\n\n岗位：{}\n岗位责任：{}",
                            request.objective,
                            selected.role.as_str(),
                            selected.responsibility
                        ),
                        owned_files: Vec::new(),
                        profile: profile.clone(),
                    })
            })
            .collect();
        let Some(assignments) = assignments else {
            decision.mode = StaffingMode::Blocked;
            decision.assignments.clear();
            decision
                .rationale
                .push("staffing decision referenced an employee missing from the roster".into());
            return StaffedTask {
                decision,
                assignments: Vec::new(),
            };
        };
        StaffedTask {
            decision,
            assignments,
        }
    }

    /// Execute a structured organization task without file-claim integration.
    /// This is suitable for read-only/scripted qualification. Mutation-capable
    /// production tasks must use the file-claim and patch-review pipeline.
    pub async fn execute_organization_task(
        &self,
        request: &OrganizationTaskRequest,
        roster: &[AgentProfile],
    ) -> anyhow::Result<OrganizationExecutionReport> {
        self.execute_organization_task_with_events(request, roster, |_| {}, None)
            .await
    }

    /// Execute a read-only organization task while publishing exact lifecycle
    /// transitions. The callback is observational and cannot change staffing,
    /// worker execution, or the deterministic completion gate.
    pub async fn execute_organization_task_with_events<F>(
        &self,
        request: &OrganizationTaskRequest,
        roster: &[AgentProfile],
        mut on_event: F,
        cancel: Option<tokio_util::sync::CancellationToken>,
    ) -> anyhow::Result<OrganizationExecutionReport>
    where
        F: FnMut(crate::event::OrganizationEvent) + Send,
    {
        use crate::event::OrganizationEvent;

        let manager = assignment_authority_label(&request.assigned_by);
        on_event(OrganizationEvent::TaskStarted {
            task_id: request.task_id.clone(),
            manager: manager.clone(),
            collaboration: collaboration_label(request.collaboration).into(),
        });
        let mut staffed = self.staff_organization_task(request, roster);
        for assignment in &staffed.decision.assignments {
            on_event(OrganizationEvent::EmployeeAssigned {
                task_id: request.task_id.clone(),
                employee: assignment.employee.clone(),
                role: assignment.role.as_str().to_string(),
                responsibility: assignment.responsibility.clone(),
                reports_to: manager.clone(),
            });
        }
        if staffed.decision.mode == StaffingMode::Direct {
            on_event(OrganizationEvent::TaskFinished {
                task_id: request.task_id.clone(),
                status: "awaiting_manager".into(),
                reason: Some("no specialist role was requested".into()),
            });
            return Ok(OrganizationExecutionReport {
                task_id: request.task_id.clone(),
                assigned_by: request.assigned_by.clone(),
                target: request.target.clone(),
                collaboration: request.collaboration,
                status: OrganizationExecutionStatus::AwaitingManager,
                staffing: staffed.decision,
                employee_reports: Vec::new(),
                execution_error: None,
                trace_events: Vec::new(),
            });
        }
        if !staffed.decision.can_execute() || staffed.assignments.is_empty() {
            on_event(OrganizationEvent::TaskFinished {
                task_id: request.task_id.clone(),
                status: "blocked".into(),
                reason: organization_block_reason(&staffed.decision),
            });
            return Ok(OrganizationExecutionReport {
                task_id: request.task_id.clone(),
                assigned_by: request.assigned_by.clone(),
                target: request.target.clone(),
                collaboration: request.collaboration,
                status: OrganizationExecutionStatus::Blocked,
                staffing: staffed.decision,
                employee_reports: Vec::new(),
                execution_error: None,
                trace_events: Vec::new(),
            });
        }
        if let Some((worker, tool)) = staffed.assignments.iter().find_map(|assignment| {
            self.organization_unsafe_tool(assignment)
                .map(|tool| (assignment.worker_name.clone(), tool))
        }) {
            staffed.decision.mode = StaffingMode::Blocked;
            staffed.decision.rationale.push(format!(
                "read-only organization executor blocked worker '{worker}' tool '{tool}'; mutation-capable tasks require the file-claim and patch-review pipeline"
            ));
            on_event(OrganizationEvent::TaskFinished {
                task_id: request.task_id.clone(),
                status: "blocked".into(),
                reason: staffed.decision.rationale.last().cloned(),
            });
            return Ok(OrganizationExecutionReport {
                task_id: request.task_id.clone(),
                assigned_by: request.assigned_by.clone(),
                target: request.target.clone(),
                collaboration: request.collaboration,
                status: OrganizationExecutionStatus::Blocked,
                staffing: staffed.decision,
                employee_reports: Vec::new(),
                execution_error: None,
                trace_events: Vec::new(),
            });
        }

        let mut employee_reports = Vec::new();
        let mut trace_events = Vec::new();
        let mut execution_error = None;
        let mut handoff_summaries = Vec::new();
        let mut previous_employee: Option<String> = None;
        for assignment in &staffed.assignments {
            // Check for graceful cancellation between workers.
            if let Some(ref ct) = cancel {
                if ct.is_cancelled() {
                    execution_error = Some("cancelled by user".into());
                    break;
                }
            }
            let mut executable = assignment.clone();
            if request.collaboration == CollaborationMode::SequentialHandoff
                && !handoff_summaries.is_empty()
            {
                executable
                    .task_description
                    .push_str("\n\n前序员工的已提交摘要（仅作交接上下文，必须独立核对）：\n");
                executable
                    .task_description
                    .push_str(&handoff_summaries.join("\n"));
                if let Some(from_employee) = previous_employee.as_ref() {
                    on_event(OrganizationEvent::Handoff {
                        task_id: request.task_id.clone(),
                        from_employee: from_employee.clone(),
                        to_employee: executable.worker_name.clone(),
                    });
                }
            }
            trace_events.push(worker_started_event(
                &executable,
                Some(request.task_id.clone()),
            ));
            on_event(OrganizationEvent::EmployeeWorking {
                task_id: request.task_id.clone(),
                employee: executable.worker_name.clone(),
                role: executable.profile.specialty.role.as_str().to_string(),
            });
            match self.execute_assignment(&executable).await {
                Ok(report) => {
                    // Treat non-success outcomes as worker failures so the
                    // sequential-handoff loop stops instead of continuing to
                    // subsequent workers with potentially invalid state.
                    if !report.success {
                        trace_events.push(worker_completed_event(
                            &executable,
                            Some(request.task_id.clone()),
                            false,
                            "failed",
                            0,
                        ));
                        execution_error = if cancel.as_ref().map_or(false, |c| c.is_cancelled()) {
                            Some("cancelled by user".into())
                        } else {
                            Some(format!(
                                "worker '{}' failed: {:?}",
                                executable.worker_name,
                                report.outcome,
                            ))
                        };
                        on_event(OrganizationEvent::EmployeeReported {
                            task_id: request.task_id.clone(),
                            employee: executable.worker_name.clone(),
                            role: executable.profile.specialty.role.as_str().to_string(),
                            outcome: "failed".into(),
                            success: false,
                        });
                        break;
                    }
                    trace_events.push(worker_completed_event(
                        &executable,
                        Some(request.task_id.clone()),
                        report.success,
                        report_status(&report),
                        report.trace_events.len(),
                    ));
                    let assignment_contract = staffed
                        .decision
                        .assignments
                        .iter()
                        .find(|selected| selected.employee == report.worker)
                        .cloned()
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "worker '{}' reported without a staffing assignment",
                                report.worker
                            )
                        })?;
                    handoff_summaries.push(format!(
                        "- {}（{}）：{}",
                        report.worker,
                        assignment_contract.role.as_str(),
                        report.summary
                    ));
                    previous_employee = Some(report.worker.clone());
                    on_event(OrganizationEvent::EmployeeReported {
                        task_id: request.task_id.clone(),
                        employee: report.worker.clone(),
                        role: assignment_contract.role.as_str().to_string(),
                        outcome: report_status(&report).into(),
                        success: report.success,
                    });
                    employee_reports.push(EmployeeWorkReport {
                        assignment: assignment_contract,
                        reports_to: request.assigned_by.clone(),
                        report,
                    });
                }
                Err(error) => {
                    trace_events.push(worker_completed_event(
                        &executable,
                        Some(request.task_id.clone()),
                        false,
                        "failed",
                        0,
                    ));
                    execution_error = if cancel.as_ref().map_or(false, |c| c.is_cancelled()) {
                        Some("cancelled by user".into())
                    } else {
                        Some(format!(
                            "worker '{}' failed: {error}",
                            executable.worker_name
                        ))
                    };
                    on_event(OrganizationEvent::EmployeeReported {
                        task_id: request.task_id.clone(),
                        employee: executable.worker_name.clone(),
                        role: executable.profile.specialty.role.as_str().to_string(),
                        outcome: "failed".into(),
                        success: false,
                    });
                    break;
                }
            }
        }

        if matches!(request.assigned_by, AssignmentAuthority::Manager { .. }) {
            on_event(OrganizationEvent::ManagerReviewing {
                task_id: request.task_id.clone(),
                manager,
            });
        }
        let reports = employee_reports
            .iter()
            .map(|employee_report| employee_report.report.clone())
            .collect::<Vec<_>>();
        let status = match classify_worker_reports(&reports, execution_error.as_deref()) {
            WorkerExecutionStatus::Completed => OrganizationExecutionStatus::Completed,
            WorkerExecutionStatus::Submitted => OrganizationExecutionStatus::Submitted,
            WorkerExecutionStatus::WorkerFailed => OrganizationExecutionStatus::WorkerFailed,
            WorkerExecutionStatus::BlockedBeforeExecution
            | WorkerExecutionStatus::CompletedWithReviewFindings => {
                OrganizationExecutionStatus::Blocked
            }
        };
        let terminal_status = match status {
            OrganizationExecutionStatus::AwaitingManager => "awaiting_manager",
            OrganizationExecutionStatus::Blocked => "blocked",
            OrganizationExecutionStatus::Completed => "accepted",
            OrganizationExecutionStatus::Submitted => "submitted",
            OrganizationExecutionStatus::WorkerFailed => "worker_failed",
        };
        on_event(OrganizationEvent::TaskFinished {
            task_id: request.task_id.clone(),
            status: terminal_status.into(),
            reason: execution_error.clone(),
        });
        Ok(OrganizationExecutionReport {
            task_id: request.task_id.clone(),
            assigned_by: request.assigned_by.clone(),
            target: request.target.clone(),
            collaboration: request.collaboration,
            status,
            staffing: staffed.decision,
            employee_reports,
            execution_error,
            trace_events,
        })
    }

    /// Execute a mutation-capable organization task through explicit file
    /// ownership, deeplossless claims, deterministic parent review, and
    /// parent-owned PatchEngine application. Patch proposals are structured
    /// inputs to this boundary; this method does not infer operations from
    /// free-form worker text.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_organization_mutation_task<C: FileClaimCoordinator>(
        &self,
        request: &OrganizationTaskRequest,
        roster: &[AgentProfile],
        file_scopes: Vec<OrganizationFileScope>,
        proposals: Vec<WorkerPatchProposal>,
        manager: impl Into<String>,
        coordinator: &C,
        conv_id: i64,
        operation: &str,
        engine: &mut PatchEngine,
    ) -> anyhow::Result<OrganizationMutationReport> {
        let manager = manager.into();
        let mut staffed = self.staff_organization_task(request, roster);
        if staffed.decision.mode == StaffingMode::Direct {
            return Ok(organization_mutation_without_pipeline(
                request,
                staffed.decision,
                manager,
                ManagerAcceptanceStatus::AwaitingManager,
                "no specialist mutation was dispatched; manager action is still required",
                Vec::new(),
            ));
        }
        if !staffed.decision.can_execute() || staffed.assignments.is_empty() {
            return Ok(organization_mutation_without_pipeline(
                request,
                staffed.decision,
                manager,
                ManagerAcceptanceStatus::Blocked,
                "staffing contract blocked mutation execution",
                Vec::new(),
            ));
        }

        let contract_errors = apply_organization_mutation_contract(
            &mut staffed.assignments,
            &file_scopes,
            &proposals,
        );
        if !contract_errors.is_empty() {
            staffed.decision.mode = StaffingMode::Blocked;
            staffed.decision.rationale.extend(contract_errors.clone());
            return Ok(organization_mutation_without_pipeline(
                request,
                staffed.decision,
                manager,
                ManagerAcceptanceStatus::Blocked,
                "mutation ownership or proposal contract is invalid",
                contract_errors,
            ));
        }

        let (concurrent, sequential_handoff) = match request.collaboration {
            CollaborationMode::Independent => (true, false),
            CollaborationMode::SequentialHandoff => (false, true),
        };
        let pipeline = self
            .execute_worker_patch_pipeline_mode(
                Some(request.task_id.clone()),
                staffed.assignments,
                coordinator,
                conv_id,
                operation,
                concurrent,
                sequential_handoff,
                proposals,
                engine,
            )
            .await?;

        let employee_reports = pipeline
            .execution
            .reports
            .iter()
            .filter_map(|report| {
                staffed
                    .decision
                    .assignments
                    .iter()
                    .find(|assignment| assignment.employee == report.worker)
                    .cloned()
                    .map(|assignment| EmployeeWorkReport {
                        assignment,
                        reports_to: request.assigned_by.clone(),
                        report: report.clone(),
                    })
            })
            .collect::<Vec<_>>();
        let (status, summary, reasons) = manager_acceptance_from_pipeline(&pipeline);

        Ok(OrganizationMutationReport {
            task_id: request.task_id.clone(),
            staffing: staffed.decision,
            employee_reports,
            pipeline: Some(pipeline),
            manager_acceptance: ManagerAcceptanceReport {
                manager,
                status,
                summary,
                reasons,
            },
        })
    }

    /// Execute a mutation task whose proposals must be submitted by each
    /// worker through `submit_patch_proposal`. Free-form report text is never
    /// parsed or guessed into patch operations.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_organization_mutation_from_workers<C: FileClaimCoordinator>(
        &self,
        request: &OrganizationTaskRequest,
        roster: &[AgentProfile],
        file_scopes: Vec<OrganizationFileScope>,
        manager: impl Into<String>,
        coordinator: &C,
        conv_id: i64,
        operation: &str,
        engine: &mut PatchEngine,
    ) -> anyhow::Result<OrganizationMutationReport> {
        let manager = manager.into();
        let mut staffed = self.staff_organization_task(request, roster);
        if staffed.decision.mode == StaffingMode::Direct {
            return Ok(organization_mutation_without_pipeline(
                request,
                staffed.decision,
                manager,
                ManagerAcceptanceStatus::AwaitingManager,
                "no specialist mutation was dispatched; manager action is still required",
                Vec::new(),
            ));
        }
        if !staffed.decision.can_execute() || staffed.assignments.is_empty() {
            return Ok(organization_mutation_without_pipeline(
                request,
                staffed.decision,
                manager,
                ManagerAcceptanceStatus::Blocked,
                "staffing contract blocked mutation execution",
                Vec::new(),
            ));
        }

        let scope_errors = apply_organization_file_scopes(&mut staffed.assignments, &file_scopes);
        if !scope_errors.is_empty() {
            staffed.decision.mode = StaffingMode::Blocked;
            staffed.decision.rationale.extend(scope_errors.clone());
            return Ok(organization_mutation_without_pipeline(
                request,
                staffed.decision,
                manager,
                ManagerAcceptanceStatus::Blocked,
                "mutation ownership contract is invalid",
                scope_errors,
            ));
        }

        self.ensure_worker_limit(staffed.assignments.len())?;
        let claim_report = self
            .claim_worker_files(&staffed.assignments, coordinator, conv_id, operation)
            .await?;
        let collector =
            std::sync::Arc::new(std::sync::Mutex::new(PatchProposalCollector::default()));
        let (concurrent, sequential_handoff) = match request.collaboration {
            CollaborationMode::Independent => (true, false),
            CollaborationMode::SequentialHandoff => (false, true),
        };
        let (reports, execution_error, trace_events) =
            if !claim_report.local_overlaps.is_empty() || !claim_report.conflicts.is_empty() {
                (Vec::new(), None, Vec::new())
            } else {
                self.execute_claimed_assignments(
                    &staffed.assignments,
                    concurrent,
                    sequential_handoff,
                    Some(collector.clone()),
                    Some(request.task_id.as_str()),
                )
                .await
            };
        let conflicts = self.detect_conflicts(&reports);
        let ownership_violations = self.detect_ownership_violations(&staffed.assignments, &reports);
        let mut execution = WorkerExecutionReport {
            status: if !claim_report.local_overlaps.is_empty() || !claim_report.conflicts.is_empty()
            {
                WorkerExecutionStatus::BlockedBeforeExecution
            } else {
                classify_worker_reports(&reports, execution_error.as_deref())
            },
            reports,
            release_failures: claim_report.release_failures.clone(),
            claim_report,
            conflicts,
            ownership_violations,
            execution_error,
            trace_events,
        };
        if execution.status != WorkerExecutionStatus::BlockedBeforeExecution
            && execution.status != WorkerExecutionStatus::WorkerFailed
            && (!execution.conflicts.is_empty()
                || !execution.ownership_violations.is_empty()
                || !execution.release_failures.is_empty())
        {
            execution.status = WorkerExecutionStatus::CompletedWithReviewFindings;
        }

        let (proposals, mut proposal_errors) = {
            let collector = collector.lock().unwrap();
            (
                collector.proposals.values().cloned().collect::<Vec<_>>(),
                collector.errors.clone(),
            )
        };
        if execution.status != WorkerExecutionStatus::BlockedBeforeExecution {
            proposal_errors.extend(validate_organization_proposals(
                &staffed.assignments,
                &proposals,
            ));
        }
        if !proposal_errors.is_empty() {
            let proposal_error = format!(
                "structured patch proposal submission failed: {}",
                proposal_errors.join("; ")
            );
            execution.execution_error = Some(match execution.execution_error.take() {
                Some(existing) => format!("{existing}; {proposal_error}"),
                None => proposal_error,
            });
            execution.status = WorkerExecutionStatus::WorkerFailed;
        }

        let pipeline = self
            .finish_worker_patch_pipeline(
                Some(request.task_id.clone()),
                staffed.assignments,
                coordinator,
                proposals,
                engine,
                execution,
            )
            .await;
        let employee_reports = pipeline
            .execution
            .reports
            .iter()
            .filter_map(|report| {
                staffed
                    .decision
                    .assignments
                    .iter()
                    .find(|assignment| assignment.employee == report.worker)
                    .cloned()
                    .map(|assignment| EmployeeWorkReport {
                        assignment,
                        reports_to: request.assigned_by.clone(),
                        report: report.clone(),
                    })
            })
            .collect::<Vec<_>>();
        let (status, summary, reasons) = manager_acceptance_from_pipeline(&pipeline);
        Ok(OrganizationMutationReport {
            task_id: request.task_id.clone(),
            staffing: staffed.decision,
            employee_reports,
            pipeline: Some(pipeline),
            manager_acceptance: ManagerAcceptanceReport {
                manager,
                status,
                summary,
                reasons,
            },
        })
    }

    /// Return the first tool that makes a profile ineligible for the read-only
    /// organization entrypoint. An empty tool list means all runtime tools and
    /// is therefore usually ineligible when mutation tools are installed.
    pub fn organization_read_only_blocker(&self, profile: &AgentProfile) -> Option<String> {
        let registry = if profile.tool_names.is_empty() {
            self.runtime.registry.clone()
        } else {
            self.runtime.registry.select(
                &profile
                    .tool_names
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            )
        };
        registry
            .as_tool_specs()
            .into_iter()
            .find(|spec| !spec.read_only || spec.side_effect != crate::tool::SideEffect::ReadOnly)
            .map(|spec| spec.name)
    }

    fn organization_unsafe_tool(&self, assignment: &WorkerAssignment) -> Option<String> {
        self.organization_read_only_blocker(&assignment.profile)
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

    /// Execute the bounded two-worker review pipeline used by the desktop
    /// review entrypoint. This is one specialized workflow, not the general
    /// employee organization/router. The analyst collects evidence without
    /// running verification; the verifier owns fresh validation.
    pub async fn execute_review_pipeline(
        &self,
        goal: &str,
        analyst_profile: AgentProfile,
        verifier_profile: AgentProfile,
        session_id: &str,
    ) -> anyhow::Result<LeadReviewReport> {
        self.execute_review_pipeline_with_events(
            goal,
            analyst_profile,
            verifier_profile,
            session_id,
            |_| {},
        )
        .await
    }

    /// Run the desktop review pipeline and report exact organization state
    /// transitions as they happen. The callback is observational only and is
    /// never used to decide execution or acceptance.
    pub async fn execute_review_pipeline_with_events<F>(
        &self,
        goal: &str,
        mut analyst_profile: AgentProfile,
        mut verifier_profile: AgentProfile,
        session_id: &str,
        mut on_event: F,
    ) -> anyhow::Result<LeadReviewReport>
    where
        F: FnMut(crate::event::OrganizationEvent) + Send,
    {
        use crate::event::OrganizationEvent;

        self.ensure_worker_limit(2)?;
        analyst_profile.verification_policy =
            crate::agent::profile::VerificationPolicy::NotRequired;
        verifier_profile.verification_policy = crate::agent::profile::VerificationPolicy::Required;
        let manager = "中书".to_string();
        on_event(OrganizationEvent::TaskStarted {
            task_id: session_id.to_string(),
            manager: manager.clone(),
            collaboration: "sequential_handoff".into(),
        });
        for (profile, responsibility) in [
            (&analyst_profile, "收集事实并定位风险"),
            (&verifier_profile, "独立复核并取得新鲜验证证据"),
        ] {
            on_event(OrganizationEvent::EmployeeAssigned {
                task_id: session_id.to_string(),
                employee: profile.name.clone(),
                role: profile.specialty.role.as_str().to_string(),
                responsibility: responsibility.into(),
                reports_to: manager.clone(),
            });
        }
        let session_id = Some(session_id.to_string());
        let analyst_assignment = WorkerAssignment {
            worker_name: analyst_profile.name.clone(),
            task_description: format!(
                "作为分析员工审查以下任务，只收集事实、定位风险并提交报告，不修改文件。不要运行测试或其他验证；即使原始任务要求验证，也由后续验证员工负责。读取必要证据后立即提交分析报告：\n\n{goal}"
            ),
            owned_files: Vec::new(),
            profile: analyst_profile,
        };
        let mut trace_events = vec![worker_started_event(
            &analyst_assignment,
            session_id.clone(),
        )];
        on_event(OrganizationEvent::EmployeeWorking {
            task_id: session_id.clone().unwrap_or_default(),
            employee: analyst_assignment.worker_name.clone(),
            role: analyst_assignment
                .profile
                .specialty
                .role
                .as_str()
                .to_string(),
        });
        let analyst = match self.execute_assignment(&analyst_assignment).await {
            Ok(report) => report,
            Err(error) => {
                on_event(OrganizationEvent::EmployeeReported {
                    task_id: session_id.clone().unwrap_or_default(),
                    employee: analyst_assignment.worker_name.clone(),
                    role: analyst_assignment
                        .profile
                        .specialty
                        .role
                        .as_str()
                        .to_string(),
                    outcome: "failed".into(),
                    success: false,
                });
                on_event(OrganizationEvent::TaskFinished {
                    task_id: session_id.clone().unwrap_or_default(),
                    status: "worker_failed".into(),
                    reason: Some(format!(
                        "employee '{}' failed",
                        analyst_assignment.worker_name
                    )),
                });
                return Err(error);
            }
        };
        trace_events.push(worker_completed_event(
            &analyst_assignment,
            session_id.clone(),
            analyst.success,
            report_status(&analyst),
            analyst.trace_events.len(),
        ));
        on_event(OrganizationEvent::EmployeeReported {
            task_id: session_id.clone().unwrap_or_default(),
            employee: analyst_assignment.worker_name.clone(),
            role: analyst_assignment
                .profile
                .specialty
                .role
                .as_str()
                .to_string(),
            outcome: report_status(&analyst).into(),
            success: analyst.success,
        });

        let verifier_assignment = WorkerAssignment {
            worker_name: verifier_profile.name.clone(),
            task_description: format!(
                "作为验证员工，独立复核分析报告并运行与任务直接相关的验证。不得修改文件；若无法获得新鲜验证证据，必须明确说明未验证。\n\n原始任务：\n{goal}\n\n分析员工报告：\n{}",
                analyst.findings
            ),
            owned_files: Vec::new(),
            profile: verifier_profile,
        };
        on_event(OrganizationEvent::Handoff {
            task_id: session_id.clone().unwrap_or_default(),
            from_employee: analyst_assignment.worker_name.clone(),
            to_employee: verifier_assignment.worker_name.clone(),
        });
        trace_events.push(worker_started_event(
            &verifier_assignment,
            session_id.clone(),
        ));
        on_event(OrganizationEvent::EmployeeWorking {
            task_id: session_id.clone().unwrap_or_default(),
            employee: verifier_assignment.worker_name.clone(),
            role: verifier_assignment
                .profile
                .specialty
                .role
                .as_str()
                .to_string(),
        });
        let verifier = match self.execute_assignment(&verifier_assignment).await {
            Ok(report) => report,
            Err(error) => {
                on_event(OrganizationEvent::EmployeeReported {
                    task_id: session_id.clone().unwrap_or_default(),
                    employee: verifier_assignment.worker_name.clone(),
                    role: verifier_assignment
                        .profile
                        .specialty
                        .role
                        .as_str()
                        .to_string(),
                    outcome: "failed".into(),
                    success: false,
                });
                on_event(OrganizationEvent::TaskFinished {
                    task_id: session_id.clone().unwrap_or_default(),
                    status: "worker_failed".into(),
                    reason: Some(format!(
                        "employee '{}' failed",
                        verifier_assignment.worker_name
                    )),
                });
                return Err(error);
            }
        };
        trace_events.push(worker_completed_event(
            &verifier_assignment,
            session_id.clone(),
            verifier.success,
            report_status(&verifier),
            verifier.trace_events.len(),
        ));
        on_event(OrganizationEvent::EmployeeReported {
            task_id: session_id.clone().unwrap_or_default(),
            employee: verifier_assignment.worker_name.clone(),
            role: verifier_assignment
                .profile
                .specialty
                .role
                .as_str()
                .to_string(),
            outcome: report_status(&verifier).into(),
            success: verifier.success,
        });
        on_event(OrganizationEvent::ManagerReviewing {
            task_id: session_id.clone().unwrap_or_default(),
            manager,
        });

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
        let (status, recovery) = review_pipeline_outcome(analyst.outcome, verifier.outcome);
        let terminal_status = match status {
            WorkerExecutionStatus::Completed => "accepted",
            WorkerExecutionStatus::Submitted => "submitted",
            WorkerExecutionStatus::WorkerFailed => "worker_failed",
            WorkerExecutionStatus::BlockedBeforeExecution => "blocked",
            WorkerExecutionStatus::CompletedWithReviewFindings => "review_findings",
        };
        on_event(OrganizationEvent::TaskFinished {
            task_id: session_id.clone().unwrap_or_default(),
            status: terminal_status.into(),
            reason: acceptance_reasons.first().cloned(),
        });

        Ok(LeadReviewReport {
            status,
            recovery,
            analyst,
            verifier,
            acceptance_reasons,
            trace_events,
        })
    }

    /// Backward-compatible name for callers compiled against the original
    /// review workflow API. New code should use [`Self::execute_review_pipeline`]
    /// so it is not confused with general employee staffing and delegation.
    #[deprecated(note = "use execute_review_pipeline")]
    pub async fn execute_review_handoff(
        &self,
        goal: &str,
        analyst_profile: AgentProfile,
        verifier_profile: AgentProfile,
        session_id: &str,
    ) -> anyhow::Result<LeadReviewReport> {
        self.execute_review_pipeline(goal, analyst_profile, verifier_profile, session_id)
            .await
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
            false,
            true,
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
        self.execute_with_file_claims_mode(
            assignments,
            coordinator,
            conv_id,
            operation,
            true,
            false,
            true,
            None,
        )
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
            false,
            true,
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
        sequential_handoff: bool,
        release_after_execution: bool,
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
            .execute_claimed_assignments(
                &assignments,
                concurrent,
                sequential_handoff,
                None,
                session_id,
            )
            .await;

        let conflicts = self.detect_conflicts(&reports);
        let ownership_violations = self.detect_ownership_violations(&assignments, &reports);
        let mut release_failures = claim_report.release_failures.clone();
        if release_after_execution {
            release_failures.extend(
                self.release_worker_file_claims(&claim_report.active_claims, coordinator)
                    .await,
            );
        }

        let base_status = classify_worker_reports(&reports, execution_error.as_deref());
        let status = if base_status == WorkerExecutionStatus::WorkerFailed {
            base_status
        } else if !conflicts.is_empty()
            || !ownership_violations.is_empty()
            || !release_failures.is_empty()
        {
            WorkerExecutionStatus::CompletedWithReviewFindings
        } else {
            base_status
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
        sequential_handoff: bool,
        proposal_collector: Option<std::sync::Arc<std::sync::Mutex<PatchProposalCollector>>>,
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
                let proposal_collector = proposal_collector.clone();
                async move {
                    let _permit = sem.acquire().await.expect("semaphore closed");
                    self.execute_assignment_with_proposal_collector(assignment, proposal_collector)
                        .await
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
        let mut handoff_summaries = Vec::new();
        for assignment in assignments {
            let mut executable = assignment.clone();
            if sequential_handoff && !handoff_summaries.is_empty() {
                executable
                    .task_description
                    .push_str("\n\n前序员工的已提交摘要（仅作交接上下文，必须独立核对）：\n");
                executable
                    .task_description
                    .push_str(&handoff_summaries.join("\n"));
            }
            trace_events.push(worker_started_event(&executable, session_id.clone()));
            match self
                .execute_assignment_with_proposal_collector(&executable, proposal_collector.clone())
                .await
            {
                Ok(report) => {
                    trace_events.push(worker_completed_event(
                        &executable,
                        session_id.clone(),
                        report.success,
                        report_status(&report),
                        report.trace_events.len(),
                    ));
                    handoff_summaries.push(format!("- {}：{}", report.worker, report.summary));
                    reports.push(report);
                }
                Err(error) => {
                    trace_events.push(worker_completed_event(
                        &executable,
                        session_id.clone(),
                        false,
                        "failed",
                        0,
                    ));
                    return (
                        reports,
                        Some(format!(
                            "worker '{}' failed: {error}",
                            executable.worker_name
                        )),
                        trace_events,
                    );
                }
            }
        }
        (reports, None, trace_events)
    }

    async fn execute_assignment(&self, assignment: &WorkerAssignment) -> anyhow::Result<Report> {
        self.execute_assignment_with_proposal_collector(assignment, None)
            .await
    }

    async fn execute_assignment_with_proposal_collector(
        &self,
        assignment: &WorkerAssignment,
        proposal_collector: Option<std::sync::Arc<std::sync::Mutex<PatchProposalCollector>>>,
    ) -> anyhow::Result<Report> {
        let task = crate::task::Task {
            id: worker_task_id(assignment),
            source: "orchestrator".into(),
            tool: "agent".into(),
            arguments: serde_json::json!({
                "task": assignment.task_description,
            }),
        };

        let Some(collector) = proposal_collector else {
            return Worker::execute(&self.runtime, &assignment.profile, task, None).await;
        };

        let mut runtime = self.runtime.clone();
        runtime.registry = runtime
            .registry
            .restrict_to_paths(&assignment.owned_files, &[])
            .register(SubmitPatchProposalTool {
                worker: assignment.worker_name.clone(),
                collector,
            });
        let mut profile = assignment.profile.clone();
        if !profile.tool_names.is_empty()
            && !profile
                .tool_names
                .iter()
                .any(|name| name == SUBMIT_PATCH_PROPOSAL_TOOL)
        {
            profile.tool_names.push(SUBMIT_PATCH_PROPOSAL_TOOL.into());
        }
        profile.system_prompt.push_str(
            "\n\nYou must submit exactly one patch proposal through the submit_patch_proposal tool. Do not place a patch only in free-form text. The tool records a proposal for manager review and does not modify files.",
        );
        // Do not spend the worker's full step budget repeatedly asking for
        // verification when it refuses the required submission tool. Parent
        // merge review still requires a CompletedVerified report, so this
        // short-circuit does not weaken acceptance.
        profile.verification_policy = crate::agent::profile::VerificationPolicy::NotRequired;
        Worker::execute(&runtime, &profile, task, None).await
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
            false,
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
            false,
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
        sequential_handoff: bool,
        proposals: Vec<WorkerPatchProposal>,
        engine: &mut PatchEngine,
    ) -> anyhow::Result<WorkerPatchPipelineReport> {
        let execution = self
            .execute_with_file_claims_mode(
                assignments.clone(),
                coordinator,
                conv_id,
                operation,
                concurrent,
                sequential_handoff,
                false,
                session_id.as_deref(),
            )
            .await?;
        Ok(self
            .finish_worker_patch_pipeline(
                session_id,
                assignments,
                coordinator,
                proposals,
                engine,
                execution,
            )
            .await)
    }

    async fn finish_worker_patch_pipeline<C: FileClaimCoordinator>(
        &self,
        session_id: Option<String>,
        assignments: Vec<WorkerAssignment>,
        coordinator: &C,
        proposals: Vec<WorkerPatchProposal>,
        engine: &mut PatchEngine,
        mut execution: WorkerExecutionReport,
    ) -> WorkerPatchPipelineReport {
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
        let deferred_release_failures = self
            .release_worker_file_claims(&execution.claim_report.active_claims, coordinator)
            .await;
        execution.release_failures.extend(deferred_release_failures);
        let status = if merge_review.status != WorkerMergeStatus::Approved {
            WorkerPatchPipelineStatus::Blocked
        } else if !apply_report.passed() {
            WorkerPatchPipelineStatus::ApplyFailed
        } else if execution.release_failures.is_empty() {
            WorkerPatchPipelineStatus::Applied
        } else {
            WorkerPatchPipelineStatus::AppliedWithReviewFindings
        };

        WorkerPatchPipelineReport {
            status,
            execution,
            merge_review,
            apply_report,
        }
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
                let outcome = match coordinator
                    .claim_file(&assignment.worker_name, &file_path, operation, conv_id)
                    .await
                {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        let cleanup_failures = self
                            .release_worker_file_claims(&active_claims, coordinator)
                            .await;
                        let cleanup = if cleanup_failures.is_empty() {
                            "previously acquired claims were released".to_string()
                        } else {
                            format!(
                                "{} previously acquired claim(s) could not be released",
                                cleanup_failures.len()
                            )
                        };
                        anyhow::bail!(
                            "failed to claim '{}' for worker '{}': {error}; {cleanup}",
                            file.display(),
                            assignment.worker_name
                        );
                    }
                };
                match outcome {
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

fn review_pipeline_outcome(
    analyst: crate::agent::RunOutcome,
    verifier: crate::agent::RunOutcome,
) -> (WorkerExecutionStatus, ReviewPipelineRecovery) {
    let analyst_submitted = matches!(
        analyst,
        crate::agent::RunOutcome::CompletedVerified | crate::agent::RunOutcome::CompletedUnverified
    );
    let verifier_verified = verifier == crate::agent::RunOutcome::CompletedVerified;
    let verifier_failed = matches!(
        verifier,
        crate::agent::RunOutcome::Failed
            | crate::agent::RunOutcome::BudgetExhausted
            | crate::agent::RunOutcome::Interrupted
    );
    let recovery = if analyst_submitted {
        ReviewPipelineRecovery::NotNeeded
    } else if verifier_verified {
        ReviewPipelineRecovery::Succeeded
    } else {
        ReviewPipelineRecovery::Failed
    };
    let status = if !analyst_submitted || verifier_failed {
        WorkerExecutionStatus::WorkerFailed
    } else if verifier_verified {
        WorkerExecutionStatus::Completed
    } else {
        WorkerExecutionStatus::Submitted
    };
    (status, recovery)
}

fn assignment_authority_label(authority: &AssignmentAuthority) -> String {
    match authority {
        AssignmentAuthority::Manager { manager } => manager.clone(),
        AssignmentAuthority::User => "用户".into(),
    }
}

fn collaboration_label(collaboration: CollaborationMode) -> &'static str {
    match collaboration {
        CollaborationMode::Independent => "independent",
        CollaborationMode::SequentialHandoff => "sequential_handoff",
    }
}

fn organization_block_reason(decision: &StaffingDecision) -> Option<String> {
    decision
        .rationale
        .first()
        .cloned()
        .or_else(|| decision.unfilled.first().map(|item| item.reason.clone()))
}

fn classify_worker_reports(
    reports: &[Report],
    execution_error: Option<&str>,
) -> WorkerExecutionStatus {
    let has_hard_failure = reports.iter().any(|report| {
        !matches!(
            report.outcome,
            crate::agent::RunOutcome::CompletedVerified
                | crate::agent::RunOutcome::CompletedUnverified
        )
    });
    if execution_error.is_some() || has_hard_failure {
        WorkerExecutionStatus::WorkerFailed
    } else if reports
        .iter()
        .any(|report| report.outcome == crate::agent::RunOutcome::CompletedUnverified)
    {
        WorkerExecutionStatus::Submitted
    } else {
        WorkerExecutionStatus::Completed
    }
}

fn apply_organization_mutation_contract(
    assignments: &mut [WorkerAssignment],
    file_scopes: &[OrganizationFileScope],
    proposals: &[WorkerPatchProposal],
) -> Vec<String> {
    let mut errors = apply_organization_file_scopes(assignments, file_scopes);
    if errors.is_empty() {
        errors.extend(validate_organization_proposals(assignments, proposals));
    }
    errors
}

fn apply_organization_file_scopes(
    assignments: &mut [WorkerAssignment],
    file_scopes: &[OrganizationFileScope],
) -> Vec<String> {
    let selected = assignments
        .iter()
        .map(|assignment| assignment.worker_name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut errors = Vec::new();
    let mut scopes = std::collections::BTreeMap::<&str, Vec<PathBuf>>::new();
    for scope in file_scopes {
        if scopes
            .insert(
                scope.employee.as_str(),
                normalize_owned_files(&scope.owned_files),
            )
            .is_some()
        {
            errors.push(format!(
                "employee '{}' has duplicate file scope entries",
                scope.employee
            ));
        }
        if !selected.contains(scope.employee.as_str()) {
            errors.push(format!(
                "file scope references unselected employee '{}'",
                scope.employee
            ));
        }
        if scope.owned_files.is_empty()
            || scope
                .owned_files
                .iter()
                .any(|path| normalize_path(path).as_os_str().is_empty())
        {
            errors.push(format!(
                "mutation employee '{}' must have at least one non-empty owned file",
                scope.employee
            ));
        }
    }
    for employee in &selected {
        if !scopes.contains_key(employee) {
            errors.push(format!(
                "selected mutation employee '{employee}' has no file scope"
            ));
        }
    }

    if errors.is_empty() {
        for assignment in assignments {
            assignment.owned_files = scopes
                .get(assignment.worker_name.as_str())
                .cloned()
                .expect("validated scope for every selected employee");
        }
    }
    errors
}

fn validate_organization_proposals(
    assignments: &[WorkerAssignment],
    proposals: &[WorkerPatchProposal],
) -> Vec<String> {
    let selected = assignments
        .iter()
        .map(|assignment| assignment.worker_name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let scopes = assignments
        .iter()
        .map(|assignment| {
            (
                assignment.worker_name.as_str(),
                normalize_owned_files(&assignment.owned_files),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut errors = Vec::new();

    let mut proposal_workers = std::collections::BTreeSet::new();
    for proposal in proposals {
        if !proposal_workers.insert(proposal.worker.as_str()) {
            errors.push(format!(
                "employee '{}' submitted more than one patch proposal",
                proposal.worker
            ));
        }
        if !selected.contains(proposal.worker.as_str()) {
            errors.push(format!(
                "patch proposal references unselected employee '{}'",
                proposal.worker
            ));
        }
        if proposal.summary.trim().is_empty() {
            errors.push(format!(
                "employee '{}' submitted an empty patch summary",
                proposal.worker
            ));
        }
        if proposal.files.is_empty() || proposal.operations.is_empty() {
            errors.push(format!(
                "employee '{}' must submit named files and patch operations",
                proposal.worker
            ));
        }
        if let Some(owned_files) = scopes.get(proposal.worker.as_str()) {
            let proposal_files = normalize_owned_files(&proposal.files);
            for file in &proposal_files {
                if !owned_files
                    .iter()
                    .any(|owned| path_matches_owned(file, owned))
                {
                    errors.push(format!(
                        "employee '{}' proposed file {} outside its mutation scope",
                        proposal.worker,
                        file.display()
                    ));
                }
            }
            for operation in &proposal.operations {
                let path = normalize_path(&operation.path().to_path_buf());
                if !proposal_files
                    .iter()
                    .any(|file| path_matches_owned(&path, file))
                    || !owned_files
                        .iter()
                        .any(|owned| path_matches_owned(&path, owned))
                {
                    errors.push(format!(
                        "employee '{}' proposed operation on {} outside its declared files or mutation scope",
                        proposal.worker,
                        path.display()
                    ));
                }
            }
        }
    }
    for employee in &selected {
        if !proposal_workers.contains(employee) {
            errors.push(format!(
                "selected mutation employee '{employee}' submitted no patch proposal"
            ));
        }
    }
    errors
}

fn manager_acceptance_from_pipeline(
    pipeline: &WorkerPatchPipelineReport,
) -> (ManagerAcceptanceStatus, String, Vec<String>) {
    match pipeline.status {
        WorkerPatchPipelineStatus::Applied => (
            ManagerAcceptanceStatus::Accepted,
            format!(
                "accepted {} employee proposal(s); applied {} patch operation(s)",
                pipeline.merge_review.decisions.len(),
                pipeline.apply_report.applied.len()
            ),
            Vec::new(),
        ),
        WorkerPatchPipelineStatus::AppliedWithReviewFindings => (
            ManagerAcceptanceStatus::AppliedWithReleaseFailures,
            "patches were applied, but one or more file claims could not be released".into(),
            pipeline
                .execution
                .release_failures
                .iter()
                .map(|failure| {
                    format!(
                        "failed to release {} for {}: {}",
                        failure.file.display(),
                        failure.worker,
                        failure.message
                    )
                })
                .collect(),
        ),
        WorkerPatchPipelineStatus::Blocked => {
            let mut reasons = pipeline.merge_review.blockers.clone();
            for decision in &pipeline.merge_review.decisions {
                reasons.extend(
                    decision
                        .reasons
                        .iter()
                        .map(|reason| format!("{}: {reason}", decision.proposal.worker)),
                );
            }
            (
                ManagerAcceptanceStatus::Blocked,
                "manager did not accept the mutation plan; no parent patch was applied".into(),
                reasons,
            )
        }
        WorkerPatchPipelineStatus::ApplyFailed => (
            ManagerAcceptanceStatus::ApplyFailed,
            "manager approved the proposals, but parent patch application failed or only partially applied; inspect applied operations and failures".into(),
            pipeline
                .apply_report
                .failures
                .iter()
                .map(|failure| failure.message.clone())
                .collect(),
        ),
    }
}

fn organization_mutation_without_pipeline(
    request: &OrganizationTaskRequest,
    staffing: StaffingDecision,
    manager: String,
    status: ManagerAcceptanceStatus,
    summary: impl Into<String>,
    reasons: Vec<String>,
) -> OrganizationMutationReport {
    OrganizationMutationReport {
        task_id: request.task_id.clone(),
        staffing,
        employee_reports: Vec::new(),
        pipeline: None,
        manager_acceptance: ManagerAcceptanceReport {
            manager,
            status,
            summary: summary.into(),
            reasons,
        },
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
    use crate::agent::llm::{
        ChatCompletionResponse, FinalChoice, FunctionCall, LlmProvider, Role, ToolCall,
    };
    use crate::agent::AgentBudget;
    use crate::harness::architecture::index::FileIndex;
    use crate::tool::{Tool, ToolOutput, ToolRegistry};
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
        pub claim_error_files: BTreeSet<String>,
        pub missing_releases: BTreeSet<String>,
        pub claimed: Mutex<Vec<(String, String)>>,
        pub released: Mutex<Vec<(String, String)>>,
    }
    struct ReleaseObservationCoordinator {
        workspace_root: PathBuf,
        released_contents: Mutex<Vec<String>>,
    }

    #[derive(Clone)]
    struct ReviewScriptedProvider {
        analyst_fails: bool,
    }

    #[derive(Clone)]
    struct OrganizationScriptedProvider {
        calls: Arc<AtomicUsize>,
    }

    struct FailingReadTool;
    struct PassingShellTool;
    struct SystemChangingScreenshotTool;

    #[async_trait]
    impl Tool for FailingReadTool {
        fn name(&self) -> &str {
            "read_file"
        }

        fn description(&self) -> &str {
            "scripted failing read"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"}}})
        }

        async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
            ToolOutput::error("scripted analyst read failure")
        }
    }

    #[async_trait]
    impl Tool for PassingShellTool {
        fn name(&self) -> &str {
            "shell"
        }

        fn description(&self) -> &str {
            "scripted verification"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type":"object","properties":{"command":{"type":"string"}}})
        }

        async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
            ToolOutput::success(serde_json::json!({"exit_code": 0, "stdout": "1 passed"}))
        }
    }

    #[async_trait]
    impl Tool for SystemChangingScreenshotTool {
        fn name(&self) -> &str {
            "screenshot"
        }

        fn description(&self) -> &str {
            "scripted screenshot with system side effects"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type":"object","properties":{}})
        }

        async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
            panic!("unsafe organization tool must be blocked before execution")
        }
    }

    #[async_trait]
    impl LlmProvider for ReviewScriptedProvider {
        async fn chat(
            &self,
            request: ChatCompletionRequest,
        ) -> anyhow::Result<ChatCompletionResponse> {
            let system = request
                .messages
                .iter()
                .find(|message| message.role == Role::System)
                .map(|message| message.content.as_str())
                .unwrap_or_default();
            let has_tool_result = request
                .messages
                .iter()
                .any(|message| message.role == Role::Tool);
            let message = if system.contains("ROLE=analyst") {
                if self.analyst_fails {
                    Message::assistant_with_tools(
                        "",
                        vec![ToolCall {
                            id: format!("analyst-read-{}", request.messages.len()),
                            call_type: "function".into(),
                            function: FunctionCall {
                                name: "read_file".into(),
                                arguments: r#"{"path":"missing"}"#.into(),
                            },
                        }],
                    )
                } else {
                    Message::assistant("analysis evidence submitted")
                }
            } else if system.contains("ROLE=verifier") && !has_tool_result {
                Message::assistant_with_tools(
                    "",
                    vec![ToolCall {
                        id: "verification-test".into(),
                        call_type: "function".into(),
                        function: FunctionCall {
                            name: "shell".into(),
                            arguments: r#"{"command":"cargo test"}"#.into(),
                        },
                    }],
                )
            } else {
                Message::assistant("verification completed with fresh passing evidence")
            };
            Ok(ChatCompletionResponse {
                choices: vec![FinalChoice {
                    message,
                    finish_reason: Some("stop".into()),
                }],
                usage: None,
            })
        }

        async fn stream_chat(
            &self,
            _request: ChatCompletionRequest,
            _on_event: Box<dyn FnMut(crate::agent::llm::StreamEvent) + Send>,
        ) -> anyhow::Result<()> {
            anyhow::bail!("streaming is not used in scripted review tests")
        }

        fn model_name(&self) -> &str {
            "review-scripted"
        }

        fn change_model(&self, _model: &str) -> Arc<dyn LlmProvider> {
            Arc::new(self.clone())
        }
    }

    #[async_trait]
    impl LlmProvider for OrganizationScriptedProvider {
        async fn chat(
            &self,
            request: ChatCompletionRequest,
        ) -> anyhow::Result<ChatCompletionResponse> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let system = request
                .messages
                .iter()
                .find(|message| message.role == Role::System)
                .map(|message| message.content.as_str())
                .unwrap_or_default();
            let user_context = request
                .messages
                .iter()
                .filter(|message| message.role == Role::User)
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let has_tool_result = request
                .messages
                .iter()
                .any(|message| message.role == Role::Tool);
            let called_tools = request
                .messages
                .iter()
                .filter_map(|message| message.tool_calls.as_ref())
                .flatten()
                .map(|call| call.function.name.as_str())
                .collect::<Vec<_>>();
            if system.contains("ROLE=mutation_receiver")
                && !user_context.contains("mutation proposal independently verified")
            {
                anyhow::bail!("mutation receiver did not receive the previous employee handoff");
            }
            let message = if system.contains("ROLE=generated_missing") {
                Message::assistant("free-form patch text is intentionally not accepted")
            } else if system.contains("ROLE=generated")
                && !called_tools.contains(&SUBMIT_PATCH_PROPOSAL_TOOL)
            {
                if !request.tools.as_ref().is_some_and(|tools| {
                    tools
                        .iter()
                        .any(|tool| tool.function.name == SUBMIT_PATCH_PROPOSAL_TOOL)
                }) {
                    anyhow::bail!("proposal submission tool was not exposed to worker");
                }
                Message::assistant_with_tools(
                    "",
                    vec![ToolCall {
                        id: "generated-proposal".into(),
                        call_type: "function".into(),
                        function: FunctionCall {
                            name: SUBMIT_PATCH_PROPOSAL_TOOL.into(),
                            arguments: serde_json::json!({
                                "summary": "update generated copy",
                                "files": ["copy.txt"],
                                "verification_commands": ["cargo test"],
                                "operations": [{
                                    "type": "replace",
                                    "path": "copy.txt",
                                    "old_text": "old",
                                    "new_text": "new",
                                    "replace_all": false
                                }]
                            })
                            .to_string(),
                        },
                    }],
                )
            } else if system.contains("ROLE=generated") && !called_tools.contains(&"shell") {
                Message::assistant_with_tools(
                    "",
                    vec![ToolCall {
                        id: "generated-verification".into(),
                        call_type: "function".into(),
                        function: FunctionCall {
                            name: "shell".into(),
                            arguments: r#"{"command":"cargo test"}"#.into(),
                        },
                    }],
                )
            } else if system.contains("ROLE=generated") {
                Message::assistant("generated proposal submitted and verified")
            } else if system.contains("ROLE=mutation") && !has_tool_result {
                Message::assistant_with_tools(
                    "",
                    vec![ToolCall {
                        id: "mutation-verification".into(),
                        call_type: "function".into(),
                        function: FunctionCall {
                            name: "shell".into(),
                            arguments: r#"{"command":"cargo test"}"#.into(),
                        },
                    }],
                )
            } else if system.contains("ROLE=mutation") {
                Message::assistant("mutation proposal independently verified")
            } else if system.contains("ROLE=accountant") {
                Message::assistant("forecast ready: projected closing cash CNY 740000")
            } else if system.contains("ROLE=treasury") {
                if !user_context.contains("forecast ready") {
                    anyhow::bail!("treasury reviewer did not receive the accountant handoff");
                }
                Message::assistant("liquidity policy breach: escalate the CNY 160000 shortfall")
            } else if system.contains("ROLE=writer") {
                Message::assistant("copy review submitted")
            } else {
                anyhow::bail!("unknown scripted organization role")
            };
            Ok(ChatCompletionResponse {
                choices: vec![FinalChoice {
                    message,
                    finish_reason: Some("stop".into()),
                }],
                usage: None,
            })
        }

        async fn stream_chat(
            &self,
            _request: ChatCompletionRequest,
            _on_event: Box<dyn FnMut(crate::agent::llm::StreamEvent) + Send>,
        ) -> anyhow::Result<()> {
            anyhow::bail!("streaming is not used in scripted organization tests")
        }

        fn model_name(&self) -> &str {
            "organization-scripted"
        }

        fn change_model(&self, _model: &str) -> Arc<dyn LlmProvider> {
            Arc::new(self.clone())
        }
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
                claim_error_files: BTreeSet::new(),
                missing_releases: BTreeSet::new(),
                claimed: Mutex::new(Vec::new()),
                released: Mutex::new(Vec::new()),
            }
        }

        pub fn with_conflict(mut self, file_path: &str) -> Self {
            self.conflict_files.insert(test_file_key(file_path));
            self
        }

        pub fn with_claim_error(mut self, file_path: &str) -> Self {
            self.claim_error_files.insert(test_file_key(file_path));
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

    impl ReleaseObservationCoordinator {
        fn new(workspace_root: impl Into<PathBuf>) -> Self {
            Self {
                workspace_root: workspace_root.into(),
                released_contents: Mutex::new(Vec::new()),
            }
        }

        fn released_contents(&self) -> Vec<String> {
            self.released_contents.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl FileClaimCoordinator for ReleaseObservationCoordinator {
        async fn claim_file(
            &self,
            agent_id: &str,
            file_path: &str,
            _operation: &str,
            conv_id: i64,
        ) -> anyhow::Result<DeeplosslessFileClaimOutcome> {
            Ok(DeeplosslessFileClaimOutcome::Claimed {
                claim: crate::integration::DeeplosslessFileClaimResult {
                    status: "claimed".into(),
                    agent_id: agent_id.to_string(),
                    file_path: file_path.to_string(),
                    conv_id,
                },
            })
        }

        async fn release_file(
            &self,
            _agent_id: &str,
            file_path: &str,
        ) -> anyhow::Result<DeeplosslessFileReleaseOutcome> {
            let snapshot = std::fs::read_to_string(self.workspace_root.join(file_path))?;
            self.released_contents.lock().unwrap().push(snapshot);
            Ok(DeeplosslessFileReleaseOutcome::Released {
                release: crate::integration::DeeplosslessFileReleaseResult {
                    status: "released".into(),
                    file_path: file_path.to_string(),
                },
            })
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
            if self.claim_error_files.contains(&file_key) {
                anyhow::bail!("scripted claim transport failure");
            }
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
    async fn claim_worker_files_releases_acquired_claims_on_coordinator_error() {
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
        let coordinator = MockFileClaimCoordinator::new().with_claim_error("src\\b.rs");
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());

        let error = orch
            .claim_worker_files(&assignments, &coordinator, 7, "edit")
            .await
            .expect_err("coordinator error must remain observable");

        assert!(error.to_string().contains("transport failure"));
        assert!(error.to_string().contains("were released"));
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
        let assignments = (0..=DEFAULT_MAX_WORKERS_PER_TASK)
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

    #[test]
    fn orchestrator_limit_matches_organization_staffing_policy() {
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        assert_eq!(orch.max_concurrent_workers, DEFAULT_MAX_WORKERS_PER_TASK);
    }

    #[test]
    fn staff_task_converts_valid_role_decision_to_bounded_assignments() {
        use crate::agent::{EmployeeCapability, EmployeeRole, RoleRequirement, StaffingRequest};

        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let roster = vec![
            dummy_profile("frontend").with_specialty(
                EmployeeRole::frontend(),
                vec![EmployeeCapability::ui_implementation()],
                "UI",
            ),
            dummy_profile("backend").with_specialty(
                EmployeeRole::backend(),
                vec![EmployeeCapability::api_implementation()],
                "API",
            ),
            dummy_profile("writer").with_specialty(
                EmployeeRole::writer(),
                vec![EmployeeCapability::product_copy()],
                "copy",
            ),
        ];
        let request = StaffingRequest {
            objective: "ship settings".into(),
            requirements: vec![
                RoleRequirement::required(EmployeeRole::backend(), "implement API"),
                RoleRequirement::required(EmployeeRole::frontend(), "implement UI"),
                RoleRequirement::required(EmployeeRole::writer(), "write copy"),
            ],
            max_workers: None,
        };

        let staffed = orch.staff_task(&request, &roster);

        assert!(staffed.decision.can_execute());
        assert_eq!(staffed.assignments.len(), 3);
        assert!(staffed.assignments[0]
            .task_description
            .contains("组织目标：ship settings"));
    }

    #[test]
    fn staff_task_never_exposes_partial_assignments_for_blocked_decision() {
        use crate::agent::{EmployeeRole, RoleRequirement, StaffingRequest};

        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let roster =
            vec![dummy_profile("backend").with_specialty(EmployeeRole::backend(), vec![], "API")];
        let request = StaffingRequest {
            objective: "ship full stack".into(),
            requirements: vec![
                RoleRequirement::required(EmployeeRole::backend(), "API"),
                RoleRequirement::required(EmployeeRole::frontend(), "UI"),
            ],
            max_workers: None,
        };

        let staffed = orch.staff_task(&request, &roster);

        assert!(!staffed.decision.can_execute());
        assert!(staffed.assignments.is_empty());
        assert_eq!(staffed.decision.assignments.len(), 1);
    }

    #[tokio::test]
    async fn organization_task_executes_dynamic_finance_roles_with_bounded_handoff() {
        use crate::agent::{
            AssignmentAuthority, CollaborationMode, EmployeeCapability, EmployeeRole,
            OrganizationTaskRequest, RoleRequirement, StaffingRequest, VerificationPolicy,
        };

        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = AgentRuntime::new(
            OrganizationScriptedProvider {
                calls: calls.clone(),
            },
            ToolRegistry::new(),
            "organization-scripted",
            AgentBudget::default(),
        );
        let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
        let roster = vec![
            AgentProfile::new(
                "accountant",
                "ROLE=accountant",
                vec![],
                AgentBudget::default(),
            )
            .with_specialty(
                EmployeeRole::new("management_accountant"),
                vec![EmployeeCapability::new("cash_flow_forecasting")],
                "cash flow",
            )
            .with_verification_policy(VerificationPolicy::NotRequired),
            AgentProfile::new("treasury", "ROLE=treasury", vec![], AgentBudget::default())
                .with_specialty(
                    EmployeeRole::new("treasury_reviewer"),
                    vec![EmployeeCapability::new("liquidity_policy_review")],
                    "liquidity policy",
                )
                .with_verification_policy(VerificationPolicy::NotRequired),
        ];
        let request = OrganizationTaskRequest::manager_selected(
            "finance-quarter",
            "zhongshu",
            StaffingRequest {
                objective: "forecast and review liquidity".into(),
                requirements: vec![
                    RoleRequirement::required(
                        EmployeeRole::new("management_accountant"),
                        "prepare forecast",
                    )
                    .with_capabilities(vec![EmployeeCapability::new("cash_flow_forecasting")]),
                    RoleRequirement::required(
                        EmployeeRole::new("treasury_reviewer"),
                        "review policy",
                    )
                    .with_capabilities(vec![EmployeeCapability::new("liquidity_policy_review")]),
                ],
                max_workers: Some(2),
            },
        )
        .with_collaboration(CollaborationMode::SequentialHandoff);

        let mut organization_events = Vec::new();
        let report = orchestrator
            .execute_organization_task_with_events(&request, &roster, |event| {
                organization_events.push(event);
            }, None)
            .await
            .expect("scripted organization execution");

        assert_eq!(report.status, OrganizationExecutionStatus::Submitted);
        assert_eq!(report.employee_reports.len(), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            report.employee_reports[1].reports_to,
            AssignmentAuthority::manager("zhongshu")
        );
        assert!(report.employee_reports[1]
            .report
            .findings
            .contains("policy breach"));
        assert_eq!(report.trace_events.len(), 4);
        assert_eq!(organization_events.len(), 10);
        assert!(matches!(
            &organization_events[5],
            crate::event::OrganizationEvent::Handoff { from_employee, to_employee, .. }
                if from_employee == "accountant" && to_employee == "treasury"
        ));
        assert!(matches!(
            &organization_events[8],
            crate::event::OrganizationEvent::ManagerReviewing { manager, .. }
                if manager == "zhongshu"
        ));
        assert!(matches!(
            &organization_events[9],
            crate::event::OrganizationEvent::TaskFinished { status, .. }
                if status == "submitted"
        ));
    }

    #[tokio::test]
    async fn user_can_assign_one_matching_employee_and_receive_its_report_directly() {
        use crate::agent::{
            AssignmentAuthority, EmployeeCapability, EmployeeRole, OrganizationTaskRequest,
            RoleRequirement, StaffingRequest, VerificationPolicy,
        };

        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = AgentRuntime::new(
            OrganizationScriptedProvider {
                calls: calls.clone(),
            },
            ToolRegistry::new(),
            "organization-scripted",
            AgentBudget::default(),
        );
        let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
        let writer = |name: &str| {
            AgentProfile::new(name, "ROLE=writer", vec![], AgentBudget::default())
                .with_specialty(
                    EmployeeRole::writer(),
                    vec![EmployeeCapability::product_copy()],
                    "copy",
                )
                .with_verification_policy(VerificationPolicy::NotRequired)
        };
        let roster = vec![writer("writer-a"), writer("writer-b")];
        let request = OrganizationTaskRequest::user_to_employee(
            "chairman-copy",
            "writer-b",
            StaffingRequest {
                objective: "review customer notice".into(),
                requirements: vec![RoleRequirement::required(
                    EmployeeRole::writer(),
                    "review copy",
                )
                .with_capabilities(vec![EmployeeCapability::product_copy()])],
                max_workers: Some(1),
            },
        );

        let report = orchestrator
            .execute_organization_task(&request, &roster)
            .await
            .expect("user direct assignment");

        assert_eq!(report.status, OrganizationExecutionStatus::Submitted);
        assert_eq!(report.employee_reports.len(), 1);
        assert_eq!(report.employee_reports[0].report.worker, "writer-b");
        assert_eq!(
            report.employee_reports[0].reports_to,
            AssignmentAuthority::User
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn user_direct_assignment_does_not_bypass_employee_capabilities() {
        use crate::agent::{
            EmployeeCapability, EmployeeRole, OrganizationTaskRequest, RoleRequirement,
            StaffingRequest, VerificationPolicy,
        };

        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = AgentRuntime::new(
            OrganizationScriptedProvider {
                calls: calls.clone(),
            },
            ToolRegistry::new(),
            "organization-scripted",
            AgentBudget::default(),
        );
        let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
        let roster = vec![
            AgentProfile::new(
                "accountant",
                "ROLE=accountant",
                vec![],
                AgentBudget::default(),
            )
            .with_specialty(
                EmployeeRole::new("management_accountant"),
                vec![EmployeeCapability::new("cash_flow_forecasting")],
                "finance",
            )
            .with_verification_policy(VerificationPolicy::NotRequired),
            AgentProfile::new("writer", "ROLE=writer", vec![], AgentBudget::default())
                .with_specialty(EmployeeRole::writer(), vec![], "copy")
                .with_verification_policy(VerificationPolicy::NotRequired),
        ];
        let request = OrganizationTaskRequest::user_to_employee(
            "bad-direct-target",
            "writer",
            StaffingRequest {
                objective: "prepare forecast".into(),
                requirements: vec![RoleRequirement::required(
                    EmployeeRole::new("management_accountant"),
                    "prepare forecast",
                )
                .with_capabilities(vec![EmployeeCapability::new("cash_flow_forecasting")])],
                max_workers: Some(1),
            },
        );

        let report = orchestrator
            .execute_organization_task(&request, &roster)
            .await
            .expect("mismatch should be a visible blocked report");

        assert_eq!(report.status, OrganizationExecutionStatus::Blocked);
        assert!(report.employee_reports.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(report.staffing.rationale[0].contains("did not match"));
    }

    #[tokio::test]
    async fn read_only_organization_executor_blocks_mutation_capable_tools_before_model_call() {
        use crate::agent::{
            EmployeeRole, OrganizationTaskRequest, RoleRequirement, StaffingRequest,
            VerificationPolicy,
        };

        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = AgentRuntime::new(
            OrganizationScriptedProvider {
                calls: calls.clone(),
            },
            ToolRegistry::new().register(PassingShellTool),
            "organization-scripted",
            AgentBudget::default(),
        );
        let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
        let roster = vec![AgentProfile::new(
            "writer",
            "ROLE=writer",
            vec!["shell".into()],
            AgentBudget::default(),
        )
        .with_specialty(EmployeeRole::writer(), vec![], "copy")
        .with_verification_policy(VerificationPolicy::NotRequired)];
        let request = OrganizationTaskRequest::manager_selected(
            "unsafe-read-only-entry",
            "zhongshu",
            StaffingRequest {
                objective: "write copy".into(),
                requirements: vec![RoleRequirement::required(
                    EmployeeRole::writer(),
                    "write copy",
                )],
                max_workers: Some(1),
            },
        );

        let report = orchestrator
            .execute_organization_task(&request, &roster)
            .await
            .expect("unsafe tool should become a blocked report");

        assert_eq!(report.status, OrganizationExecutionStatus::Blocked);
        assert!(report.employee_reports.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(report.staffing.rationale.last().unwrap().contains("shell"));
    }

    #[tokio::test]
    async fn read_only_organization_executor_checks_side_effect_not_only_read_only_flag() {
        use crate::agent::{
            EmployeeRole, OrganizationTaskRequest, RoleRequirement, StaffingRequest,
            VerificationPolicy,
        };

        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = AgentRuntime::new(
            OrganizationScriptedProvider {
                calls: calls.clone(),
            },
            ToolRegistry::new().register(SystemChangingScreenshotTool),
            "organization-scripted",
            AgentBudget::default(),
        );
        let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
        let roster = vec![AgentProfile::new(
            "writer",
            "ROLE=writer",
            vec!["screenshot".into()],
            AgentBudget::default(),
        )
        .with_specialty(EmployeeRole::writer(), vec![], "copy")
        .with_verification_policy(VerificationPolicy::NotRequired)];
        let request = OrganizationTaskRequest::manager_selected(
            "system-changing-read-only-tool",
            "zhongshu",
            StaffingRequest {
                objective: "inspect a screen before writing copy".into(),
                requirements: vec![RoleRequirement::required(
                    EmployeeRole::writer(),
                    "inspect copy",
                )],
                max_workers: Some(1),
            },
        );

        let report = orchestrator
            .execute_organization_task(&request, &roster)
            .await
            .expect("system-changing tool should become a blocked report");

        assert_eq!(report.status, OrganizationExecutionStatus::Blocked);
        assert!(report.employee_reports.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(report
            .staffing
            .rationale
            .last()
            .unwrap()
            .contains("screenshot"));
    }

    #[tokio::test]
    async fn patch_proposal_tool_accepts_exact_schema_and_pins_worker_identity() {
        let collector = Arc::new(Mutex::new(PatchProposalCollector::default()));
        let tool = SubmitPatchProposalTool {
            worker: "writer".into(),
            collector: collector.clone(),
        };
        let output = tool
            .execute(&serde_json::json!({
                "summary": "update copy",
                "files": ["copy.txt"],
                "verification_commands": ["cargo test"],
                "operations": [{
                    "type": "replace",
                    "path": "copy.txt",
                    "old_text": "old",
                    "new_text": "new",
                    "replace_all": false
                }]
            }))
            .await;

        assert_eq!(output.status, crate::tool::ToolStatus::Success);
        let collector = collector.lock().unwrap();
        let proposal = collector.proposals.get("writer").unwrap();
        assert_eq!(proposal.worker, "writer");
        assert_eq!(proposal.files, vec![PathBuf::from("copy.txt")]);
        assert!(collector.errors.is_empty());
    }

    #[tokio::test]
    async fn patch_proposal_tool_records_malformed_and_duplicate_submissions_as_errors() {
        let collector = Arc::new(Mutex::new(PatchProposalCollector::default()));
        let tool = SubmitPatchProposalTool {
            worker: "writer".into(),
            collector: collector.clone(),
        };
        let malformed = serde_json::json!({
            "summary": "update copy",
            "files": ["copy.txt"],
            "operations": [{
                "type": "replace",
                "path": "copy.txt",
                "old_text": "old",
                "new_text": "new",
                "replace_all": false,
                "unexpected": true
            }]
        });
        let valid = serde_json::json!({
            "summary": "update copy",
            "files": ["copy.txt"],
            "operations": [{
                "type": "replace",
                "path": "copy.txt",
                "old_text": "old",
                "new_text": "new",
                "replace_all": false
            }]
        });

        let malformed_output = tool.execute(&malformed).await;
        let first_valid = tool.execute(&valid).await;
        let duplicate = tool.execute(&valid).await;

        assert_eq!(malformed_output.status, crate::tool::ToolStatus::Error);
        assert_eq!(first_valid.status, crate::tool::ToolStatus::Success);
        assert_eq!(duplicate.status, crate::tool::ToolStatus::Error);
        let collector = collector.lock().unwrap();
        assert_eq!(collector.proposals.len(), 1);
        assert_eq!(collector.errors.len(), 2);
        assert!(collector.errors[0].contains("unknown field"));
        assert!(collector.errors[1].contains("more than one"));
    }

    #[tokio::test]
    async fn organization_mutation_collects_worker_generated_proposal_without_text_parsing() {
        use crate::agent::{
            EmployeeRole, OrganizationFileScope, OrganizationTaskRequest, RoleRequirement,
            StaffingRequest, VerificationPolicy,
        };

        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("copy.txt"), "old\n").unwrap();
        let mut engine = PatchEngine::new(temp.path()).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = AgentRuntime::new(
            OrganizationScriptedProvider {
                calls: calls.clone(),
            },
            ToolRegistry::new().register(PassingShellTool),
            "organization-scripted",
            AgentBudget::default(),
        );
        let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
        let roster = vec![AgentProfile::new(
            "writer",
            "ROLE=generated",
            vec!["shell".into()],
            AgentBudget::default(),
        )
        .with_specialty(EmployeeRole::writer(), vec![], "copy")
        .with_verification_policy(VerificationPolicy::Required)];
        let request = OrganizationTaskRequest::manager_selected(
            "worker-generated-proposal",
            "zhongshu",
            StaffingRequest {
                objective: "update copy".into(),
                requirements: vec![RoleRequirement::required(EmployeeRole::writer(), "copy")],
                max_workers: Some(1),
            },
        );
        let scopes = vec![OrganizationFileScope {
            employee: "writer".into(),
            owned_files: vec![PathBuf::from("copy.txt")],
        }];
        let coordinator = MockFileClaimCoordinator::new();

        let report = orchestrator
            .execute_organization_mutation_from_workers(
                &request,
                &roster,
                scopes,
                "zhongshu",
                &coordinator,
                46,
                "edit",
                &mut engine,
            )
            .await
            .expect("worker-generated proposal pipeline");

        assert!(report.can_finalize());
        assert_eq!(
            report.manager_acceptance.status,
            ManagerAcceptanceStatus::Accepted
        );
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("copy.txt")).unwrap(),
            "new\n"
        );
        let pipeline = report.pipeline.unwrap();
        assert_eq!(pipeline.merge_review.decisions.len(), 1);
        assert_eq!(pipeline.merge_review.decisions[0].proposal.worker, "writer");
    }

    #[tokio::test]
    async fn organization_mutation_rejects_free_form_report_without_submission_tool_call() {
        use crate::agent::{
            EmployeeRole, OrganizationFileScope, OrganizationTaskRequest, RoleRequirement,
            StaffingRequest, VerificationPolicy,
        };

        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("copy.txt"), "old\n").unwrap();
        let mut engine = PatchEngine::new(temp.path()).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = AgentRuntime::new(
            OrganizationScriptedProvider {
                calls: calls.clone(),
            },
            ToolRegistry::new().register(PassingShellTool),
            "organization-scripted",
            AgentBudget::default(),
        );
        let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
        let roster = vec![AgentProfile::new(
            "writer",
            "ROLE=generated_missing",
            vec!["shell".into()],
            AgentBudget::default(),
        )
        .with_specialty(EmployeeRole::writer(), vec![], "copy")
        .with_verification_policy(VerificationPolicy::Required)];
        let request = OrganizationTaskRequest::manager_selected(
            "missing-generated-proposal",
            "zhongshu",
            StaffingRequest {
                objective: "update copy".into(),
                requirements: vec![RoleRequirement::required(EmployeeRole::writer(), "copy")],
                max_workers: Some(1),
            },
        );
        let scopes = vec![OrganizationFileScope {
            employee: "writer".into(),
            owned_files: vec![PathBuf::from("copy.txt")],
        }];
        let coordinator = MockFileClaimCoordinator::new();

        let report = orchestrator
            .execute_organization_mutation_from_workers(
                &request,
                &roster,
                scopes,
                "zhongshu",
                &coordinator,
                47,
                "edit",
                &mut engine,
            )
            .await
            .expect("missing proposal must be visible as blocked");

        assert!(!report.can_finalize());
        assert_eq!(
            report.manager_acceptance.status,
            ManagerAcceptanceStatus::Blocked
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("copy.txt")).unwrap(),
            "old\n"
        );
        assert!(report
            .pipeline
            .unwrap()
            .execution
            .execution_error
            .unwrap()
            .contains("submitted no patch proposal"));
    }

    #[tokio::test]
    async fn organization_mutation_holds_claims_through_parent_apply_and_acceptance() {
        use crate::agent::{
            CollaborationMode, EmployeeRole, OrganizationFileScope, OrganizationTaskRequest,
            RoleRequirement, StaffingRequest, VerificationPolicy,
        };

        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("frontend.txt"), "front-old\n").expect("frontend fixture");
        std::fs::write(temp.path().join("backend.txt"), "back-old\n").expect("backend fixture");
        let mut engine = PatchEngine::new(temp.path()).expect("patch engine");
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = AgentRuntime::new(
            OrganizationScriptedProvider {
                calls: calls.clone(),
            },
            ToolRegistry::new().register(PassingShellTool),
            "organization-scripted",
            AgentBudget::default(),
        );
        let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
        let roster = vec![
            AgentProfile::new(
                "frontend-employee",
                "ROLE=mutation_sender",
                vec!["shell".into()],
                AgentBudget::default(),
            )
            .with_specialty(EmployeeRole::frontend(), vec![], "frontend")
            .with_verification_policy(VerificationPolicy::Required),
            AgentProfile::new(
                "backend-employee",
                "ROLE=mutation_receiver",
                vec!["shell".into()],
                AgentBudget::default(),
            )
            .with_specialty(EmployeeRole::backend(), vec![], "backend")
            .with_verification_policy(VerificationPolicy::Required),
        ];
        let request = OrganizationTaskRequest::manager_selected(
            "organization-mutation-success",
            "zhongshu",
            StaffingRequest {
                objective: "update the frontend and backend fixtures".into(),
                requirements: vec![
                    RoleRequirement::required(EmployeeRole::frontend(), "update frontend"),
                    RoleRequirement::required(EmployeeRole::backend(), "update backend"),
                ],
                max_workers: Some(2),
            },
        )
        .with_collaboration(CollaborationMode::SequentialHandoff);
        let scopes = vec![
            OrganizationFileScope {
                employee: "frontend-employee".into(),
                owned_files: vec![PathBuf::from("frontend.txt")],
            },
            OrganizationFileScope {
                employee: "backend-employee".into(),
                owned_files: vec![PathBuf::from("backend.txt")],
            },
        ];
        let proposals = vec![
            WorkerPatchProposal::new(
                "frontend-employee",
                vec![PathBuf::from("frontend.txt")],
                "update frontend fixture",
            )
            .with_operations(vec![PatchOperation::Replace(
                crate::patch::ReplaceRequest::once("frontend.txt", "front-old", "front-new"),
            )]),
            WorkerPatchProposal::new(
                "backend-employee",
                vec![PathBuf::from("backend.txt")],
                "update backend fixture",
            )
            .with_operations(vec![PatchOperation::Replace(
                crate::patch::ReplaceRequest::once("backend.txt", "back-old", "back-new"),
            )]),
        ];
        let coordinator = ReleaseObservationCoordinator::new(temp.path());

        let report = orchestrator
            .execute_organization_mutation_task(
                &request,
                &roster,
                scopes,
                proposals,
                "zhongshu",
                &coordinator,
                42,
                "edit",
                &mut engine,
            )
            .await
            .expect("organization mutation");

        assert!(report.can_finalize());
        assert_eq!(
            report.manager_acceptance.status,
            ManagerAcceptanceStatus::Accepted
        );
        assert_eq!(report.employee_reports.len(), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("frontend.txt")).unwrap(),
            "front-new\n"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("backend.txt")).unwrap(),
            "back-new\n"
        );
        assert_eq!(
            coordinator.released_contents(),
            vec!["front-new\n".to_string(), "back-new\n".to_string()]
        );
    }

    #[tokio::test]
    async fn organization_mutation_rejects_missing_scope_before_claim_or_model_call() {
        use crate::agent::{
            EmployeeRole, OrganizationTaskRequest, RoleRequirement, StaffingRequest,
            VerificationPolicy,
        };

        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = AgentRuntime::new(
            OrganizationScriptedProvider {
                calls: calls.clone(),
            },
            ToolRegistry::new().register(PassingShellTool),
            "organization-scripted",
            AgentBudget::default(),
        );
        let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
        let roster = vec![AgentProfile::new(
            "writer",
            "ROLE=mutation_sender",
            vec!["shell".into()],
            AgentBudget::default(),
        )
        .with_specialty(EmployeeRole::writer(), vec![], "copy")
        .with_verification_policy(VerificationPolicy::Required)];
        let request = OrganizationTaskRequest::manager_selected(
            "organization-mutation-no-scope",
            "zhongshu",
            StaffingRequest {
                objective: "update copy".into(),
                requirements: vec![RoleRequirement::required(EmployeeRole::writer(), "copy")],
                max_workers: Some(1),
            },
        );
        let proposals = vec![WorkerPatchProposal::new(
            "writer",
            vec![PathBuf::from("copy.txt")],
            "update copy",
        )
        .with_operations(vec![PatchOperation::Replace(
            crate::patch::ReplaceRequest::once("copy.txt", "old", "new"),
        )])];
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("copy.txt"), "old\n").unwrap();
        let mut engine = PatchEngine::new(temp.path()).unwrap();
        let coordinator = MockFileClaimCoordinator::new();

        let report = orchestrator
            .execute_organization_mutation_task(
                &request,
                &roster,
                Vec::new(),
                proposals,
                "zhongshu",
                &coordinator,
                43,
                "edit",
                &mut engine,
            )
            .await
            .expect("invalid mutation contract is a visible report");

        assert_eq!(
            report.manager_acceptance.status,
            ManagerAcceptanceStatus::Blocked
        );
        assert!(report.pipeline.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(coordinator.claimed().is_empty());
        assert_eq!(
            std::fs::read_to_string(temp.path().join("copy.txt")).unwrap(),
            "old\n"
        );
    }

    #[test]
    fn organization_mutation_contract_rejects_out_of_scope_operation_before_execution() {
        let mut assignments = vec![WorkerAssignment {
            worker_name: "writer".into(),
            task_description: "update copy".into(),
            owned_files: Vec::new(),
            profile: dummy_profile("writer"),
        }];
        let scopes = vec![OrganizationFileScope {
            employee: "writer".into(),
            owned_files: vec![PathBuf::from("copy.txt")],
        }];
        let proposals = vec![WorkerPatchProposal::new(
            "writer",
            vec![PathBuf::from("other.txt")],
            "edit outside scope",
        )
        .with_operations(vec![PatchOperation::Replace(
            crate::patch::ReplaceRequest::once("other.txt", "old", "new"),
        )])];

        let errors = apply_organization_mutation_contract(&mut assignments, &scopes, &proposals);

        assert!(errors.iter().any(|error| error.contains("outside")));
        assert_eq!(assignments[0].owned_files, vec![PathBuf::from("copy.txt")]);
    }

    #[tokio::test]
    async fn organization_mutation_claim_conflict_blocks_before_model_and_write() {
        use crate::agent::{
            EmployeeRole, OrganizationFileScope, OrganizationTaskRequest, RoleRequirement,
            StaffingRequest, VerificationPolicy,
        };

        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = AgentRuntime::new(
            OrganizationScriptedProvider {
                calls: calls.clone(),
            },
            ToolRegistry::new().register(PassingShellTool),
            "organization-scripted",
            AgentBudget::default(),
        );
        let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
        let roster = vec![AgentProfile::new(
            "writer",
            "ROLE=mutation_sender",
            vec!["shell".into()],
            AgentBudget::default(),
        )
        .with_specialty(EmployeeRole::writer(), vec![], "copy")
        .with_verification_policy(VerificationPolicy::Required)];
        let request = OrganizationTaskRequest::manager_selected(
            "organization-mutation-conflict",
            "zhongshu",
            StaffingRequest {
                objective: "update copy".into(),
                requirements: vec![RoleRequirement::required(EmployeeRole::writer(), "copy")],
                max_workers: Some(1),
            },
        );
        let scopes = vec![OrganizationFileScope {
            employee: "writer".into(),
            owned_files: vec![PathBuf::from("copy.txt")],
        }];
        let proposals = vec![WorkerPatchProposal::new(
            "writer",
            vec![PathBuf::from("copy.txt")],
            "update copy",
        )
        .with_operations(vec![PatchOperation::Replace(
            crate::patch::ReplaceRequest::once("copy.txt", "old", "new"),
        )])];
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("copy.txt"), "old\n").unwrap();
        let mut engine = PatchEngine::new(temp.path()).unwrap();
        let coordinator = MockFileClaimCoordinator::new().with_conflict("copy.txt");

        let report = orchestrator
            .execute_organization_mutation_task(
                &request,
                &roster,
                scopes,
                proposals,
                "zhongshu",
                &coordinator,
                44,
                "edit",
                &mut engine,
            )
            .await
            .expect("claim conflict is a visible blocked report");

        assert_eq!(
            report.manager_acceptance.status,
            ManagerAcceptanceStatus::Blocked
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("copy.txt")).unwrap(),
            "old\n"
        );
    }

    #[tokio::test]
    async fn organization_mutation_does_not_finalize_after_claim_release_failure() {
        use crate::agent::{
            EmployeeRole, OrganizationFileScope, OrganizationTaskRequest, RoleRequirement,
            StaffingRequest, VerificationPolicy,
        };

        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = AgentRuntime::new(
            OrganizationScriptedProvider {
                calls: calls.clone(),
            },
            ToolRegistry::new().register(PassingShellTool),
            "organization-scripted",
            AgentBudget::default(),
        );
        let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
        let roster = vec![AgentProfile::new(
            "writer",
            "ROLE=mutation_sender",
            vec!["shell".into()],
            AgentBudget::default(),
        )
        .with_specialty(EmployeeRole::writer(), vec![], "copy")
        .with_verification_policy(VerificationPolicy::Required)];
        let request = OrganizationTaskRequest::manager_selected(
            "organization-mutation-release-failure",
            "zhongshu",
            StaffingRequest {
                objective: "update copy".into(),
                requirements: vec![RoleRequirement::required(EmployeeRole::writer(), "copy")],
                max_workers: Some(1),
            },
        );
        let scopes = vec![OrganizationFileScope {
            employee: "writer".into(),
            owned_files: vec![PathBuf::from("copy.txt")],
        }];
        let proposals = vec![WorkerPatchProposal::new(
            "writer",
            vec![PathBuf::from("copy.txt")],
            "update copy",
        )
        .with_operations(vec![PatchOperation::Replace(
            crate::patch::ReplaceRequest::once("copy.txt", "old", "new"),
        )])];
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("copy.txt"), "old\n").unwrap();
        let mut engine = PatchEngine::new(temp.path()).unwrap();
        let coordinator = MockFileClaimCoordinator::new().with_missing_release("copy.txt");

        let report = orchestrator
            .execute_organization_mutation_task(
                &request,
                &roster,
                scopes,
                proposals,
                "zhongshu",
                &coordinator,
                45,
                "edit",
                &mut engine,
            )
            .await
            .expect("release failure remains visible after apply");

        assert!(!report.can_finalize());
        assert_eq!(
            report.manager_acceptance.status,
            ManagerAcceptanceStatus::AppliedWithReleaseFailures
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("copy.txt")).unwrap(),
            "new\n"
        );
    }

    #[tokio::test]
    async fn review_handoff_keeps_unverified_workers_submitted() {
        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());

        let report = orch
            .execute_review_pipeline(
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

    #[test]
    fn review_pipeline_records_recovery_without_hiding_worker_failure() {
        let (status, recovery) = review_pipeline_outcome(
            crate::agent::RunOutcome::Failed,
            crate::agent::RunOutcome::CompletedVerified,
        );

        assert_eq!(status, WorkerExecutionStatus::WorkerFailed);
        assert_eq!(recovery, ReviewPipelineRecovery::Succeeded);
    }

    #[test]
    fn review_pipeline_completes_when_both_roles_finish_their_contracts() {
        let (status, recovery) = review_pipeline_outcome(
            crate::agent::RunOutcome::CompletedUnverified,
            crate::agent::RunOutcome::CompletedVerified,
        );

        assert_eq!(status, WorkerExecutionStatus::Completed);
        assert_eq!(recovery, ReviewPipelineRecovery::NotNeeded);
    }

    #[tokio::test]
    async fn scripted_review_pipeline_completes_without_live_provider() {
        let runtime = AgentRuntime::new(
            ReviewScriptedProvider {
                analyst_fails: false,
            },
            ToolRegistry::new()
                .register(FailingReadTool)
                .register(PassingShellTool),
            "review-scripted",
            AgentBudget::default(),
        );
        let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
        let analyst = AgentProfile::new(
            "analyst",
            "ROLE=analyst",
            vec!["read_file".into()],
            AgentBudget::default(),
        );
        let verifier = AgentProfile::new(
            "verifier",
            "ROLE=verifier",
            vec!["shell".into()],
            AgentBudget::default(),
        );

        let report = orchestrator
            .execute_review_pipeline(
                "review and run verification",
                analyst,
                verifier,
                "scripted-success",
            )
            .await
            .expect("scripted review pipeline");

        assert_eq!(
            report.analyst.outcome,
            crate::agent::RunOutcome::CompletedUnverified
        );
        assert_eq!(
            report.verifier.outcome,
            crate::agent::RunOutcome::CompletedVerified
        );
        assert_eq!(report.status, WorkerExecutionStatus::Completed);
        assert_eq!(report.recovery, ReviewPipelineRecovery::NotNeeded);
    }

    #[tokio::test]
    async fn scripted_review_pipeline_emits_real_organization_transitions_in_order() {
        let runtime = AgentRuntime::new(
            ReviewScriptedProvider {
                analyst_fails: false,
            },
            ToolRegistry::new()
                .register(FailingReadTool)
                .register(PassingShellTool),
            "review-scripted",
            AgentBudget::default(),
        );
        let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
        let analyst = AgentProfile::new(
            "analyst",
            "ROLE=analyst",
            vec!["read_file".into()],
            AgentBudget::default(),
        )
        .with_specialty(crate::agent::EmployeeRole::architect(), vec![], "review");
        let verifier = AgentProfile::new(
            "verifier",
            "ROLE=verifier",
            vec!["shell".into()],
            AgentBudget::default(),
        )
        .with_specialty(crate::agent::EmployeeRole::tester(), vec![], "verification");
        let mut events = Vec::new();

        let report = orchestrator
            .execute_review_pipeline_with_events(
                "review and run verification",
                analyst,
                verifier,
                "scripted-events",
                |event| events.push(event),
            )
            .await
            .expect("scripted organization events");

        assert_eq!(report.status, WorkerExecutionStatus::Completed);
        assert_eq!(events.len(), 10);
        assert!(matches!(
            &events[0],
            crate::event::OrganizationEvent::TaskStarted { task_id, .. }
                if task_id == "scripted-events"
        ));
        assert!(matches!(
            &events[1],
            crate::event::OrganizationEvent::EmployeeAssigned { employee, role, .. }
                if employee == "analyst" && role == "architect"
        ));
        assert!(matches!(
            &events[5],
            crate::event::OrganizationEvent::Handoff { from_employee, to_employee, .. }
                if from_employee == "analyst" && to_employee == "verifier"
        ));
        assert!(matches!(
            &events[8],
            crate::event::OrganizationEvent::ManagerReviewing { manager, .. }
                if manager == "中书"
        ));
        assert!(matches!(
            &events[9],
            crate::event::OrganizationEvent::TaskFinished { status, reason: None, .. }
                if status == "accepted"
        ));
    }

    #[tokio::test]
    async fn scripted_review_pipeline_records_analyst_failure_recovery() {
        let runtime = AgentRuntime::new(
            ReviewScriptedProvider {
                analyst_fails: true,
            },
            ToolRegistry::new()
                .register(FailingReadTool)
                .register(PassingShellTool),
            "review-scripted",
            AgentBudget::default(),
        );
        let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
        let analyst = AgentProfile::new(
            "analyst",
            "ROLE=analyst",
            vec!["read_file".into()],
            AgentBudget::default(),
        );
        let verifier = AgentProfile::new(
            "verifier",
            "ROLE=verifier",
            vec!["shell".into()],
            AgentBudget::default(),
        );

        let report = orchestrator
            .execute_review_pipeline(
                "review and run verification",
                analyst,
                verifier,
                "scripted-recovery",
            )
            .await
            .expect("scripted recovery pipeline");

        assert_eq!(report.analyst.outcome, crate::agent::RunOutcome::Failed);
        assert_eq!(
            report.verifier.outcome,
            crate::agent::RunOutcome::CompletedVerified
        );
        assert_eq!(report.status, WorkerExecutionStatus::WorkerFailed);
        assert_eq!(report.recovery, ReviewPipelineRecovery::Succeeded);
        assert!(report
            .acceptance_reasons
            .iter()
            .any(|reason| reason.contains("analysis worker ended with failed")));
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
                success: true,
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

    #[tokio::test]
    async fn organization_task_cancelled_before_execution_returns_worker_failed() {
        use crate::agent::{
            AssignmentAuthority, CollaborationMode, EmployeeCapability, EmployeeRole,
            OrganizationTaskRequest, RoleRequirement, StaffingRequest, VerificationPolicy,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = AgentRuntime::new(
            OrganizationScriptedProvider {
                calls: calls.clone(),
            },
            ToolRegistry::new(),
            "organization-scripted",
            AgentBudget::default(),
        );
        let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
        let roster = vec![
            AgentProfile::new("analyst", "ROLE=analyst", vec![], AgentBudget::default())
                .with_specialty(
                    EmployeeRole::new("analyst"),
                    vec![],
                    "analysis",
                )
                .with_verification_policy(VerificationPolicy::NotRequired),
        ];
        let request = OrganizationTaskRequest::manager_selected(
            "test-cancel",
            "zhongshu",
            StaffingRequest {
                objective: "analyze".into(),
                requirements: vec![
                    RoleRequirement::required(EmployeeRole::new("analyst"), "analysis"),
                ],
                max_workers: Some(1),
            },
        );

        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();

        let report = orchestrator
            .execute_organization_task_with_events(&request, &roster, |_| {}, Some(cancel))
            .await
            .expect("cancelled execution should return Ok");

        assert_eq!(report.status, OrganizationExecutionStatus::WorkerFailed);
        assert!(report.execution_error.as_deref() == Some("cancelled by user"));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "no worker should have run");
    }

    #[tokio::test]
    async fn organization_task_sequential_handoff_stops_on_worker_failure() {
        use crate::agent::{
            AssignmentAuthority, CollaborationMode, EmployeeCapability, EmployeeRole,
            OrganizationTaskRequest, RoleRequirement, StaffingRequest, VerificationPolicy,
        };

        let runtime = AgentRuntime::new(
            ReviewScriptedProvider { analyst_fails: true },
            ToolRegistry::new(),
            "review-scripted",
            AgentBudget::default(),
        );
        let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
        let roster = vec![
            AgentProfile::new("analyst", "ROLE=analyst", vec![], AgentBudget::default())
                .with_specialty(
                    EmployeeRole::new("analyst"),
                    vec![],
                    "analysis",
                )
                .with_verification_policy(VerificationPolicy::NotRequired),
            AgentProfile::new("verifier", "ROLE=verifier", vec![], AgentBudget::default())
                .with_specialty(
                    EmployeeRole::new("reviewer"),
                    vec![],
                    "review",
                )
                .with_verification_policy(VerificationPolicy::NotRequired),
        ];
        let request = OrganizationTaskRequest::manager_selected(
            "test-handoff-fail",
            "zhongshu",
            StaffingRequest {
                objective: "analyze with handoff".into(),
                requirements: vec![
                    RoleRequirement::required(EmployeeRole::new("analyst"), "analysis"),
                    RoleRequirement::required(EmployeeRole::new("reviewer"), "review"),
                ],
                max_workers: Some(2),
            },
        )
        .with_collaboration(CollaborationMode::SequentialHandoff);

        let report = orchestrator
            .execute_organization_task_with_events(&request, &roster, |_| {}, None)
            .await
            .expect("should return Ok even with worker failure");

        assert_eq!(
            report.status,
            OrganizationExecutionStatus::WorkerFailed,
            "worker failure should propagate to organization status"
        );
        assert!(
            report.execution_error.is_some(),
            "should report which worker failed"
        );
        assert!(
            report.employee_reports.is_empty() ||
            report.employee_reports.len() < 2,
            "only the first worker may have run; the second should have been skipped"
        );
    }
}
