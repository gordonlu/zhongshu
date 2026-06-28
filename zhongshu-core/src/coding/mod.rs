use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::core::models::id;
use crate::harness::architecture::repo_intelligence::RepoIntelligenceReport;
use crate::harness::state::VerificationState;
use crate::harness::trace::event::HarnessEvent;
use crate::harness::verification::execute::{
    execute_plan, VerificationCommandRunner, VerificationExecutionReport,
};
use crate::harness::verification::plan::{
    VerificationCommand, VerificationPlan, VerificationReason,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodingSession {
    pub id: String,
    pub trace_id: String,
    pub repo_root: PathBuf,
    pub intent: CodingIntent,
    pub model: String,
    pub source: String,
    pub runtime_link: CodingRuntimeLink,
    pub plan: Option<CodingPlan>,
    pub outcome: Option<CodingOutcome>,
}

impl CodingSession {
    pub fn new(
        repo_root: impl Into<PathBuf>,
        intent: CodingIntent,
        model: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            id: id("coding-session"),
            trace_id: id("coding-trace"),
            repo_root: repo_root.into(),
            intent,
            model: model.into(),
            source: source.into(),
            runtime_link: CodingRuntimeLink::default(),
            plan: None,
            outcome: None,
        }
    }

    pub fn started_event(&self) -> HarnessEvent {
        HarnessEvent::CodingSessionStarted {
            timestamp: timestamp(),
            session_id: self.id.clone(),
            trace_id: self.trace_id.clone(),
            repo_root: self.repo_root.clone(),
            intent: self.intent.as_str().to_string(),
            model: self.model.clone(),
            source: self.source.clone(),
            deeplossless_conversation_id: self.runtime_link.deeplossless_conversation_id,
            deeplossless_replay_execution_id: self
                .runtime_link
                .deeplossless_replay_execution_id
                .clone(),
        }
    }

    pub fn link_deeplossless(
        &mut self,
        conversation_id: Option<i64>,
        replay_execution_id: Option<String>,
    ) {
        self.runtime_link.deeplossless_conversation_id = conversation_id;
        self.runtime_link.deeplossless_replay_execution_id = replay_execution_id;
    }

    pub fn set_plan(&mut self, mut plan: CodingPlan) -> Result<HarnessEvent, CodingSessionError> {
        if self.outcome.is_some() {
            return Err(CodingSessionError::SessionAlreadyFinished);
        }
        plan.validate()?;
        plan.reset_statuses();
        let event = HarnessEvent::CodingPlanCreated {
            session_id: self.id.clone(),
            step_count: plan.steps.len(),
            risk: plan.risk.as_str().to_string(),
        };
        self.plan = Some(plan);
        Ok(event)
    }

    pub fn start_step(&mut self, index: usize) -> Result<HarnessEvent, CodingSessionError> {
        if self.outcome.is_some() {
            return Err(CodingSessionError::SessionAlreadyFinished);
        }
        let session_id = self.id.clone();
        let (step_id, kind, title) = {
            let step = self.step_mut(index)?;
            if !matches!(step.status, CodingStepStatus::Planned) {
                return Err(CodingSessionError::StepNotPlanned {
                    index,
                    status: step.status.as_str().to_string(),
                });
            }
            step.status = CodingStepStatus::Running;
            (
                step.id.clone(),
                step.kind.as_str().to_string(),
                step.title.clone(),
            )
        };
        Ok(HarnessEvent::CodingStepStarted {
            session_id,
            step_id,
            kind,
            title,
        })
    }

    pub fn complete_step(
        &mut self,
        index: usize,
        status: CodingStepStatus,
    ) -> Result<HarnessEvent, CodingSessionError> {
        if !status.is_terminal() {
            return Err(CodingSessionError::NonTerminalCompletion);
        }
        if self.outcome.is_some() {
            return Err(CodingSessionError::SessionAlreadyFinished);
        }
        let session_id = self.id.clone();
        let (step_id, status_text) = {
            let step = self.step_mut(index)?;
            if !matches!(step.status, CodingStepStatus::Running) {
                return Err(CodingSessionError::StepNotRunning {
                    index,
                    status: step.status.as_str().to_string(),
                });
            }
            step.status = status;
            (step.id.clone(), step.status.as_str().to_string())
        };
        Ok(HarnessEvent::CodingStepCompleted {
            session_id,
            step_id,
            status: status_text,
        })
    }

    pub async fn execute_verification_step<R: VerificationCommandRunner + Send>(
        &mut self,
        index: usize,
        verification_state: &mut VerificationState,
        runner: &mut R,
        start_step: u32,
    ) -> Result<CodingVerificationStepReport, CodingSessionError> {
        let plan = {
            let step = self
                .plan
                .as_ref()
                .and_then(|plan| plan.steps.get(index))
                .ok_or(CodingSessionError::StepIndexOutOfRange { index })?;
            if !matches!(step.kind, CodingStepKind::Verify) {
                return Err(CodingSessionError::InvalidPlan(format!(
                    "step {index} is not a verification step"
                )));
            }
            step.verification_plan()
        };

        let started = self.start_step(index)?;
        let execution = execute_plan(verification_state, &plan, runner, start_step)
            .await
            .map_err(|error| CodingSessionError::VerificationExecution(error.to_string()))?;
        let status = if execution.passed {
            CodingStepStatus::Completed
        } else {
            CodingStepStatus::Blocked {
                reason: execution
                    .failure_summary
                    .clone()
                    .unwrap_or_else(|| "verification failed".into()),
            }
        };
        let completed = self.complete_step(index, status)?;

        let mut events = Vec::with_capacity(execution.trace_events.len() + 2);
        events.push(started);
        events.extend(execution.trace_events.iter().cloned());
        events.push(completed);

        Ok(CodingVerificationStepReport { events, execution })
    }

    pub fn record_outcome(
        &mut self,
        outcome: CodingOutcome,
    ) -> Result<HarnessEvent, CodingSessionError> {
        if self.outcome.is_some() {
            return Err(CodingSessionError::SessionAlreadyFinished);
        }
        let event = HarnessEvent::CodingOutcomeRecorded {
            timestamp: timestamp(),
            session_id: self.id.clone(),
            outcome: outcome.as_str().to_string(),
        };
        self.outcome = Some(outcome);
        Ok(event)
    }

    pub fn next_planned_step(&self) -> Option<(usize, &CodingStep)> {
        self.plan.as_ref().and_then(|plan| {
            plan.steps
                .iter()
                .enumerate()
                .find(|(_, step)| matches!(step.status, CodingStepStatus::Planned))
        })
    }

    pub fn is_finished(&self) -> bool {
        self.outcome.is_some()
    }

    pub fn snapshot(&self) -> CodingSessionSnapshot {
        let (step_count, completed_steps, running_step) = self
            .plan
            .as_ref()
            .map(|plan| {
                let completed = plan
                    .steps
                    .iter()
                    .filter(|step| matches!(step.status, CodingStepStatus::Completed))
                    .count();
                let running = plan
                    .steps
                    .iter()
                    .find(|step| matches!(step.status, CodingStepStatus::Running))
                    .map(|step| step.id.clone());
                (plan.steps.len(), completed, running)
            })
            .unwrap_or((0, 0, None));

        CodingSessionSnapshot {
            id: self.id.clone(),
            trace_id: self.trace_id.clone(),
            repo_root: self.repo_root.clone(),
            intent: self.intent.as_str().to_string(),
            model: self.model.clone(),
            source: self.source.clone(),
            step_count,
            completed_steps,
            running_step,
            outcome: self.outcome.as_ref().map(|outcome| outcome.as_str().into()),
            runtime_link: self.runtime_link.clone(),
        }
    }

    pub fn status_summary(&self) -> String {
        let snap = self.snapshot();
        match snap.outcome.as_deref() {
            Some(outcome) => format!(
                "coding session {} finished with {outcome} ({}/{})",
                snap.id, snap.completed_steps, snap.step_count
            ),
            None if snap.step_count == 0 => {
                format!("coding session {} has no plan", snap.id)
            }
            None => format!(
                "coding session {} in progress ({}/{})",
                snap.id, snap.completed_steps, snap.step_count
            ),
        }
    }

    fn step_mut(&mut self, index: usize) -> Result<&mut CodingStep, CodingSessionError> {
        let plan = self.plan.as_mut().ok_or(CodingSessionError::MissingPlan)?;
        plan.steps
            .get_mut(index)
            .ok_or(CodingSessionError::StepIndexOutOfRange { index })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodingVerificationStepReport {
    pub events: Vec<HarnessEvent>,
    pub execution: VerificationExecutionReport,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodingRuntimeLink {
    pub deeplossless_conversation_id: Option<i64>,
    pub deeplossless_replay_execution_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodingSessionSnapshot {
    pub id: String,
    pub trace_id: String,
    pub repo_root: PathBuf,
    pub intent: String,
    pub model: String,
    pub source: String,
    pub step_count: usize,
    pub completed_steps: usize,
    pub running_step: Option<String>,
    pub outcome: Option<String>,
    pub runtime_link: CodingRuntimeLink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodingIntent {
    Fix,
    Implement,
    Review,
    Explain,
    Refactor,
    Test,
    Investigate,
}

impl CodingIntent {
    pub fn as_str(&self) -> &'static str {
        match self {
            CodingIntent::Fix => "fix",
            CodingIntent::Implement => "implement",
            CodingIntent::Review => "review",
            CodingIntent::Explain => "explain",
            CodingIntent::Refactor => "refactor",
            CodingIntent::Test => "test",
            CodingIntent::Investigate => "investigate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodingPlan {
    pub summary: String,
    pub risk: CodingRisk,
    pub steps: Vec<CodingStep>,
}

impl CodingPlan {
    pub fn new(summary: impl Into<String>, risk: CodingRisk, steps: Vec<CodingStep>) -> Self {
        Self {
            summary: summary.into(),
            risk,
            steps,
        }
    }

    fn reset_statuses(&mut self) {
        for step in &mut self.steps {
            step.status = CodingStepStatus::Planned;
        }
    }

    pub fn validate(&self) -> Result<(), CodingSessionError> {
        if self.summary.trim().is_empty() {
            return Err(CodingSessionError::InvalidPlan(
                "plan summary cannot be empty".into(),
            ));
        }
        if self.steps.is_empty() {
            return Err(CodingSessionError::InvalidPlan(
                "plan must contain at least one step".into(),
            ));
        }
        for (index, step) in self.steps.iter().enumerate() {
            if step.title.trim().is_empty() {
                return Err(CodingSessionError::InvalidPlan(format!(
                    "step {index} title cannot be empty"
                )));
            }
            if matches!(step.kind, CodingStepKind::Verify) && step.verification_commands.is_empty()
            {
                return Err(CodingSessionError::InvalidPlan(format!(
                    "verify step {index} must include at least one command"
                )));
            }
        }
        Ok(())
    }

    pub fn with_repo_verification_step(mut self, report: &RepoIntelligenceReport) -> Self {
        if !report.verification.required || report.verification.commands.is_empty() {
            return self;
        }
        let commands = report
            .verification
            .commands
            .iter()
            .map(|command| command.command.clone())
            .collect();
        let step = CodingStep::new("verify changed behavior", CodingStepKind::Verify)
            .with_expected_files(report.changed_files.clone())
            .with_verification_commands(commands);
        self.steps.push(step);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodingRisk {
    Low,
    Medium,
    High,
}

impl CodingRisk {
    pub fn as_str(&self) -> &'static str {
        match self {
            CodingRisk::Low => "low",
            CodingRisk::Medium => "medium",
            CodingRisk::High => "high",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodingStep {
    pub id: String,
    pub title: String,
    pub kind: CodingStepKind,
    pub expected_files: Vec<PathBuf>,
    pub verification_commands: Vec<String>,
    pub status: CodingStepStatus,
}

impl CodingStep {
    pub fn new(title: impl Into<String>, kind: CodingStepKind) -> Self {
        Self {
            id: id("coding-step"),
            title: title.into(),
            kind,
            expected_files: Vec::new(),
            verification_commands: Vec::new(),
            status: CodingStepStatus::Planned,
        }
    }

    pub fn with_expected_files(mut self, files: Vec<PathBuf>) -> Self {
        self.expected_files = files;
        self
    }

    pub fn with_verification_commands(mut self, commands: Vec<String>) -> Self {
        self.verification_commands = commands;
        self
    }

    pub fn verification_plan(&self) -> VerificationPlan {
        if self.verification_commands.is_empty() {
            return VerificationPlan::empty();
        }
        VerificationPlan {
            required: true,
            commands: self
                .verification_commands
                .iter()
                .map(|command| {
                    VerificationCommand::new(command.clone(), VerificationReason::UserProvided)
                })
                .collect(),
            environment_notes: Vec::new(),
            fallback_commands: Vec::new(),
        }
    }

    pub fn requires_verification(&self) -> bool {
        matches!(self.kind, CodingStepKind::Edit | CodingStepKind::Verify)
            || !self.verification_commands.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodingStepKind {
    Read,
    Analyze,
    Edit,
    Verify,
    Recover,
    Review,
    Finalize,
}

impl CodingStepKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CodingStepKind::Read => "read",
            CodingStepKind::Analyze => "analyze",
            CodingStepKind::Edit => "edit",
            CodingStepKind::Verify => "verify",
            CodingStepKind::Recover => "recover",
            CodingStepKind::Review => "review",
            CodingStepKind::Finalize => "finalize",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodingStepStatus {
    Planned,
    Running,
    Completed,
    Failed { reason: String },
    Blocked { reason: String },
}

impl CodingStepStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CodingStepStatus::Planned => "planned",
            CodingStepStatus::Running => "running",
            CodingStepStatus::Completed => "completed",
            CodingStepStatus::Failed { .. } => "failed",
            CodingStepStatus::Blocked { .. } => "blocked",
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(
            self,
            CodingStepStatus::Completed
                | CodingStepStatus::Failed { .. }
                | CodingStepStatus::Blocked { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodingOutcome {
    Completed,
    Blocked { reason: String },
    Failed { reason: String },
    NeedsApproval { reason: String },
    VerificationFailed { command: String },
    Aborted { reason: String },
}

impl CodingOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            CodingOutcome::Completed => "completed",
            CodingOutcome::Blocked { .. } => "blocked",
            CodingOutcome::Failed { .. } => "failed",
            CodingOutcome::NeedsApproval { .. } => "needs_approval",
            CodingOutcome::VerificationFailed { .. } => "verification_failed",
            CodingOutcome::Aborted { .. } => "aborted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodingSessionError {
    MissingPlan,
    StepIndexOutOfRange { index: usize },
    StepNotPlanned { index: usize, status: String },
    StepNotRunning { index: usize, status: String },
    NonTerminalCompletion,
    SessionAlreadyFinished,
    InvalidPlan(String),
    VerificationExecution(String),
}

impl fmt::Display for CodingSessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodingSessionError::MissingPlan => write!(f, "coding session has no plan"),
            CodingSessionError::StepIndexOutOfRange { index } => {
                write!(f, "coding step index {index} is out of range")
            }
            CodingSessionError::StepNotPlanned { index, status } => {
                write!(
                    f,
                    "coding step index {index} cannot start from status {status}"
                )
            }
            CodingSessionError::StepNotRunning { index, status } => {
                write!(
                    f,
                    "coding step index {index} cannot complete from status {status}"
                )
            }
            CodingSessionError::NonTerminalCompletion => {
                write!(f, "coding step completion requires a terminal status")
            }
            CodingSessionError::SessionAlreadyFinished => write!(f, "coding session is finished"),
            CodingSessionError::InvalidPlan(reason) => write!(f, "invalid coding plan: {reason}"),
            CodingSessionError::VerificationExecution(reason) => {
                write!(f, "verification execution failed: {reason}")
            }
        }
    }
}

impl std::error::Error for CodingSessionError {}

fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    #[derive(Default)]
    struct MockVerificationRunner {
        outputs: Vec<crate::harness::verification::execute::VerificationCommandOutput>,
    }

    #[async_trait]
    impl VerificationCommandRunner for MockVerificationRunner {
        async fn run(
            &mut self,
            _command: &VerificationCommand,
        ) -> anyhow::Result<crate::harness::verification::execute::VerificationCommandOutput>
        {
            Ok(self.outputs.remove(0))
        }
    }

    fn verification_state() -> VerificationState {
        VerificationState {
            required: false,
            records: Vec::new(),
            last_success: None,
            last_failure: None,
            last_edit_step: 0,
            last_verify_step: 0,
            unavailable_reason: None,
        }
    }

    #[test]
    fn session_start_emits_trace_identity() {
        let session = CodingSession::new(".", CodingIntent::Fix, "deepseek-v4-flash", "test");

        let event = session.started_event();

        match event {
            HarnessEvent::CodingSessionStarted {
                session_id,
                trace_id,
                intent,
                model,
                source,
                deeplossless_conversation_id,
                deeplossless_replay_execution_id,
                ..
            } => {
                assert_eq!(session_id, session.id);
                assert_eq!(trace_id, session.trace_id);
                assert_eq!(intent, "fix");
                assert_eq!(model, "deepseek-v4-flash");
                assert_eq!(source, "test");
                assert_eq!(deeplossless_conversation_id, None);
                assert_eq!(deeplossless_replay_execution_id, None);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn plan_resets_steps_and_emits_count() {
        let mut session = CodingSession::new(".", CodingIntent::Implement, "model", "test");
        let mut step = CodingStep::new("edit file", CodingStepKind::Edit);
        step.status = CodingStepStatus::Running;
        let plan = CodingPlan::new("implement feature", CodingRisk::Medium, vec![step]);

        let event = session.set_plan(plan).unwrap();

        assert_eq!(
            session.plan.as_ref().unwrap().steps[0].status,
            CodingStepStatus::Planned
        );
        assert!(matches!(
            event,
            HarnessEvent::CodingPlanCreated {
                step_count: 1,
                risk,
                ..
            } if risk == "medium"
        ));
    }

    #[test]
    fn step_lifecycle_updates_status_and_trace_events() {
        let mut session = CodingSession::new(".", CodingIntent::Fix, "model", "test");
        let step = CodingStep::new("run tests", CodingStepKind::Verify)
            .with_verification_commands(vec!["cargo test -p zhongshu-core".into()]);
        session
            .set_plan(CodingPlan::new("fix", CodingRisk::Low, vec![step]))
            .unwrap();

        let started = session.start_step(0).unwrap();
        assert!(matches!(
            started,
            HarnessEvent::CodingStepStarted { kind, .. } if kind == "verify"
        ));
        assert_eq!(
            session.plan.as_ref().unwrap().steps[0].status,
            CodingStepStatus::Running
        );

        let completed = session
            .complete_step(0, CodingStepStatus::Completed)
            .unwrap();
        assert!(matches!(
            completed,
            HarnessEvent::CodingStepCompleted { status, .. } if status == "completed"
        ));
        assert_eq!(
            session.plan.as_ref().unwrap().steps[0].status,
            CodingStepStatus::Completed
        );
    }

    #[test]
    fn complete_rejects_non_terminal_status() {
        let mut session = CodingSession::new(".", CodingIntent::Fix, "model", "test");
        session
            .set_plan(CodingPlan::new(
                "fix",
                CodingRisk::Low,
                vec![CodingStep::new("read", CodingStepKind::Read)],
            ))
            .unwrap();

        let err = session
            .complete_step(0, CodingStepStatus::Running)
            .unwrap_err();

        assert_eq!(err, CodingSessionError::NonTerminalCompletion);
    }

    #[test]
    fn outcome_is_recorded_with_trace_event() {
        let mut session = CodingSession::new(".", CodingIntent::Review, "model", "test");

        let event = session
            .record_outcome(CodingOutcome::NeedsApproval {
                reason: "dangerous command".into(),
            })
            .unwrap();

        assert!(matches!(
            event,
            HarnessEvent::CodingOutcomeRecorded { outcome, .. } if outcome == "needs_approval"
        ));
        assert!(matches!(
            session.outcome,
            Some(CodingOutcome::NeedsApproval { .. })
        ));
    }

    #[test]
    fn runtime_link_is_included_in_snapshot() {
        let mut session = CodingSession::new(".", CodingIntent::Investigate, "model", "test");
        session.link_deeplossless(Some(42), Some("exec-1".into()));

        let snapshot = session.snapshot();

        assert_eq!(snapshot.runtime_link.deeplossless_conversation_id, Some(42));
        assert_eq!(
            snapshot
                .runtime_link
                .deeplossless_replay_execution_id
                .as_deref(),
            Some("exec-1")
        );
    }

    #[test]
    fn snapshot_reports_running_and_completed_steps() {
        let mut session = CodingSession::new(".", CodingIntent::Fix, "model", "test");
        session
            .set_plan(CodingPlan::new(
                "fix",
                CodingRisk::Low,
                vec![
                    CodingStep::new("read", CodingStepKind::Read),
                    CodingStep::new("edit", CodingStepKind::Edit),
                ],
            ))
            .unwrap();

        session.start_step(0).unwrap();
        session
            .complete_step(0, CodingStepStatus::Completed)
            .unwrap();
        session.start_step(1).unwrap();
        let running_id = session.plan.as_ref().unwrap().steps[1].id.clone();

        let snapshot = session.snapshot();

        assert_eq!(snapshot.step_count, 2);
        assert_eq!(snapshot.completed_steps, 1);
        assert_eq!(snapshot.running_step.as_deref(), Some(running_id.as_str()));
        assert!(session.status_summary().contains("in progress"));
    }

    #[test]
    fn verify_step_requires_commands() {
        let plan = CodingPlan::new(
            "verify",
            CodingRisk::Low,
            vec![CodingStep::new("verify", CodingStepKind::Verify)],
        );

        let err = plan.validate().unwrap_err();

        assert!(
            matches!(err, CodingSessionError::InvalidPlan(reason) if reason.contains("command"))
        );
    }

    #[test]
    fn session_tracks_next_planned_step() {
        let mut session = CodingSession::new(".", CodingIntent::Fix, "model", "test");
        session
            .set_plan(CodingPlan::new(
                "fix",
                CodingRisk::Low,
                vec![
                    CodingStep::new("read", CodingStepKind::Read),
                    CodingStep::new("edit", CodingStepKind::Edit),
                ],
            ))
            .unwrap();

        assert_eq!(session.next_planned_step().unwrap().0, 0);
        session.start_step(0).unwrap();
        session
            .complete_step(0, CodingStepStatus::Completed)
            .unwrap();

        assert_eq!(session.next_planned_step().unwrap().0, 1);
    }

    #[test]
    fn coding_plan_can_append_repo_verification_step() {
        let report = RepoIntelligenceReport {
            changed_files: vec![PathBuf::from("zhongshu-core/src/lib.rs")],
            affected_files: Vec::new(),
            affected_symbols: Vec::new(),
            risks: Vec::new(),
            verification: VerificationPlan::for_changes(
                &[PathBuf::from("zhongshu-core/src/lib.rs")],
                "fix bug",
            ),
            working_set: crate::harness::architecture::repo_intelligence::WorkingSet::default(),
        };

        let plan = CodingPlan::new(
            "fix",
            CodingRisk::Low,
            vec![CodingStep::new("edit", CodingStepKind::Edit)],
        )
        .with_repo_verification_step(&report);

        assert_eq!(plan.steps.len(), 2);
        assert!(matches!(plan.steps[1].kind, CodingStepKind::Verify));
        assert!(plan.steps[1]
            .verification_commands
            .contains(&"cargo test -p zhongshu-core".to_string()));
        assert!(plan.validate().is_ok());
    }

    #[test]
    fn coding_plan_does_not_append_empty_repo_verification() {
        let report = RepoIntelligenceReport {
            changed_files: Vec::new(),
            affected_files: Vec::new(),
            affected_symbols: Vec::new(),
            risks: Vec::new(),
            verification: VerificationPlan::empty(),
            working_set: crate::harness::architecture::repo_intelligence::WorkingSet::default(),
        };

        let plan = CodingPlan::new(
            "explain",
            CodingRisk::Low,
            vec![CodingStep::new("read", CodingStepKind::Read)],
        )
        .with_repo_verification_step(&report);

        assert_eq!(plan.steps.len(), 1);
    }

    #[test]
    fn finished_session_rejects_more_mutation() {
        let mut session = CodingSession::new(".", CodingIntent::Fix, "model", "test");
        session.record_outcome(CodingOutcome::Completed).unwrap();

        let err = session
            .set_plan(CodingPlan::new(
                "fix",
                CodingRisk::Low,
                vec![CodingStep::new("read", CodingStepKind::Read)],
            ))
            .unwrap_err();

        assert_eq!(err, CodingSessionError::SessionAlreadyFinished);
    }

    #[tokio::test]
    async fn execute_verification_step_completes_on_success() {
        let mut session = CodingSession::new(".", CodingIntent::Fix, "model", "test");
        let step = CodingStep::new("verify", CodingStepKind::Verify)
            .with_verification_commands(vec!["cargo check -p zhongshu-core".into()]);
        session
            .set_plan(CodingPlan::new("fix", CodingRisk::Low, vec![step]))
            .unwrap();
        let mut state = verification_state();
        let mut runner = MockVerificationRunner {
            outputs: vec![
                crate::harness::verification::execute::VerificationCommandOutput::success("ok"),
            ],
        };

        let report = session
            .execute_verification_step(0, &mut state, &mut runner, 10)
            .await
            .unwrap();

        assert!(report.execution.passed);
        assert!(matches!(
            session.plan.as_ref().unwrap().steps[0].status,
            CodingStepStatus::Completed
        ));
        assert_eq!(
            state.last_success.as_ref().map(|record| record.step),
            Some(11)
        );
        assert!(report
            .events
            .iter()
            .any(|event| matches!(event, HarnessEvent::Verification { success: true, .. })));
    }

    #[tokio::test]
    async fn execute_verification_step_blocks_on_failure() {
        let mut session = CodingSession::new(".", CodingIntent::Fix, "model", "test");
        let step = CodingStep::new("verify", CodingStepKind::Verify)
            .with_verification_commands(vec!["cargo test -p zhongshu-core".into()]);
        session
            .set_plan(CodingPlan::new("fix", CodingRisk::Low, vec![step]))
            .unwrap();
        let mut state = verification_state();
        let mut runner = MockVerificationRunner {
            outputs: vec![
                crate::harness::verification::execute::VerificationCommandOutput::failure(
                    1,
                    "test failed",
                ),
            ],
        };

        let report = session
            .execute_verification_step(0, &mut state, &mut runner, 20)
            .await
            .unwrap();

        assert!(report.execution.blocked);
        assert!(matches!(
            session.plan.as_ref().unwrap().steps[0].status,
            CodingStepStatus::Blocked { .. }
        ));
        assert_eq!(
            state.last_failure.as_ref().map(|record| record.step),
            Some(21)
        );
        assert!(report
            .events
            .iter()
            .any(|event| matches!(event, HarnessEvent::Verification { success: false, .. })));
    }
}
