use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::harness::state::VerificationState;
use crate::harness::trace::event::HarnessEvent;
use crate::harness::verification::ledger;
use crate::harness::verification::plan::{VerificationCommand, VerificationPlan};
use crate::tool::shell_semantics::ShellCommandClass;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationExecutionReport {
    pub required: bool,
    pub command_results: Vec<VerificationCommandResult>,
    pub fallback_results: Vec<VerificationCommandResult>,
    pub environment_notes: Vec<String>,
    pub passed: bool,
    pub blocked: bool,
    pub failure_summary: Option<String>,
    pub trace_events: Vec<HarnessEvent>,
}

impl VerificationExecutionReport {
    pub fn skipped() -> Self {
        Self {
            required: false,
            command_results: Vec::new(),
            fallback_results: Vec::new(),
            environment_notes: Vec::new(),
            passed: true,
            blocked: false,
            failure_summary: None,
            trace_events: Vec::new(),
        }
    }

    pub fn failed_commands(&self) -> Vec<&VerificationCommandResult> {
        self.command_results
            .iter()
            .chain(self.fallback_results.iter())
            .filter(|result| !result.success)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationCommandResult {
    pub command: VerificationCommand,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout_preview: String,
    pub stderr_preview: String,
    pub error: Option<String>,
    pub step: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationCommandOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
}

impl VerificationCommandOutput {
    pub fn success(stdout: impl Into<String>) -> Self {
        Self {
            exit_code: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
            error: None,
        }
    }

    pub fn failure(exit_code: i32, stderr: impl Into<String>) -> Self {
        Self {
            exit_code: Some(exit_code),
            stdout: String::new(),
            stderr: stderr.into(),
            error: None,
        }
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            error: Some(message.into()),
        }
    }

    fn success_flag(&self) -> bool {
        self.error.is_none() && self.exit_code == Some(0)
    }
}

#[async_trait]
pub trait VerificationCommandRunner {
    async fn run(
        &mut self,
        command: &VerificationCommand,
    ) -> anyhow::Result<VerificationCommandOutput>;
}

#[derive(Debug, Clone)]
pub struct ShellVerificationRunner {
    pub cwd: Option<std::path::PathBuf>,
}

impl ShellVerificationRunner {
    pub fn new(cwd: impl Into<std::path::PathBuf>) -> Self {
        Self {
            cwd: Some(cwd.into()),
        }
    }
}

#[async_trait]
impl VerificationCommandRunner for ShellVerificationRunner {
    async fn run(
        &mut self,
        command: &VerificationCommand,
    ) -> anyhow::Result<VerificationCommandOutput> {
        if command.class != ShellCommandClass::Verification {
            return Ok(VerificationCommandOutput::unavailable(format!(
                "command is not classified as verification: {}",
                command.command
            )));
        }

        let command_text = command.command.clone();
        let cwd = self.cwd.clone();
        tokio::task::spawn_blocking(move || run_shell_command(&command_text, cwd)).await?
    }
}

pub async fn execute_plan<R: VerificationCommandRunner + Send>(
    state: &mut VerificationState,
    plan: &VerificationPlan,
    runner: &mut R,
    start_step: u32,
) -> anyhow::Result<VerificationExecutionReport> {
    if !plan.required {
        state.required = false;
        return Ok(VerificationExecutionReport::skipped());
    }

    state.required = true;
    let mut step = start_step;
    let mut trace_events = Vec::new();
    let mut command_results = Vec::new();

    for command in &plan.commands {
        step += 1;
        let result = run_one(state, runner, command, step).await?;
        trace_events.push(verification_trace(&result));
        let failed = !result.success;
        command_results.push(result);
        if failed {
            break;
        }
    }

    let mut fallback_results = Vec::new();
    let primary_failed = command_results.iter().any(|result| !result.success);
    if primary_failed {
        for command in &plan.fallback_commands {
            step += 1;
            let result = run_one(state, runner, command, step).await?;
            trace_events.push(verification_trace(&result));
            fallback_results.push(result);
        }
    }

    let primary_passed = !plan.commands.is_empty()
        && command_results.len() == plan.commands.len()
        && command_results.iter().all(|result| result.success);
    let fallback_passed = primary_failed
        && !fallback_results.is_empty()
        && fallback_results.iter().all(|result| result.success);
    let passed = primary_passed || fallback_passed;
    let blocked = !passed;
    let failure_summary = if passed {
        None
    } else {
        command_results
            .iter()
            .chain(fallback_results.iter())
            .find(|result| !result.success)
            .map(summarize_failure)
            .or_else(|| Some("verification plan had no executable commands".into()))
    };

    Ok(VerificationExecutionReport {
        required: true,
        command_results,
        fallback_results,
        environment_notes: plan.environment_notes.clone(),
        passed,
        blocked,
        failure_summary,
        trace_events,
    })
}

async fn run_one<R: VerificationCommandRunner + Send>(
    state: &mut VerificationState,
    runner: &mut R,
    command: &VerificationCommand,
    step: u32,
) -> anyhow::Result<VerificationCommandResult> {
    let output = runner.run(command).await?;
    let success = output.success_flag();
    let result = VerificationCommandResult {
        command: command.clone(),
        success,
        exit_code: output.exit_code,
        stdout_preview: truncate_chars(&output.stdout, 2_000),
        stderr_preview: truncate_chars(&output.stderr, 2_000),
        error: output.error,
        step,
    };
    ledger::record(
        state,
        "shell",
        &result.command.command,
        result.exit_code,
        step,
    );
    if result.error.is_some() {
        state.unavailable_reason = result.error.clone();
    }
    Ok(result)
}

fn run_shell_command(
    command: &str,
    cwd: Option<std::path::PathBuf>,
) -> anyhow::Result<VerificationCommandOutput> {
    let (shell, flag) = if cfg!(target_os = "windows") {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    let mut cmd = std::process::Command::new(shell);
    cmd.arg(flag).arg(command);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let output = cmd.output()?;
    Ok(VerificationCommandOutput {
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        error: None,
    })
}

fn verification_trace(result: &VerificationCommandResult) -> HarnessEvent {
    HarnessEvent::Verification {
        command: result.command.command.clone(),
        success: result.success,
        exit_code: result.exit_code,
        step: result.step,
    }
}

fn summarize_failure(result: &VerificationCommandResult) -> String {
    if let Some(error) = &result.error {
        return format!("{} unavailable: {error}", result.command.command);
    }
    if !result.stderr_preview.is_empty() {
        return format!(
            "{} failed: {}",
            result.command.command, result.stderr_preview
        );
    }
    format!(
        "{} failed with exit code {:?}",
        result.command.command, result.exit_code
    )
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in text.chars().take(max_chars) {
        out.push(ch);
    }
    if text.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::verification::plan::{VerificationCommand, VerificationReason};

    #[derive(Default)]
    struct MockRunner {
        outputs: Vec<VerificationCommandOutput>,
        seen: Vec<String>,
    }

    #[async_trait]
    impl VerificationCommandRunner for MockRunner {
        async fn run(
            &mut self,
            command: &VerificationCommand,
        ) -> anyhow::Result<VerificationCommandOutput> {
            self.seen.push(command.command.clone());
            Ok(self.outputs.remove(0))
        }
    }

    fn state() -> VerificationState {
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

    fn command(text: &str) -> VerificationCommand {
        VerificationCommand::new(text, VerificationReason::RustCoreChange)
    }

    #[tokio::test]
    async fn executes_required_plan_and_records_success() {
        let plan = VerificationPlan {
            required: true,
            commands: vec![command("cargo test -p zhongshu-core")],
            environment_notes: Vec::new(),
            fallback_commands: Vec::new(),
        };
        let mut state = state();
        let mut runner = MockRunner {
            outputs: vec![VerificationCommandOutput::success("ok")],
            seen: Vec::new(),
        };

        let report = execute_plan(&mut state, &plan, &mut runner, 10)
            .await
            .unwrap();

        assert!(report.passed);
        assert!(!report.blocked);
        assert_eq!(state.last_success.as_ref().map(|r| r.step), Some(11));
        assert_eq!(report.trace_events.len(), 1);
        assert_eq!(runner.seen, vec!["cargo test -p zhongshu-core"]);
    }

    #[tokio::test]
    async fn stops_primary_commands_on_failure_and_runs_fallbacks() {
        let plan = VerificationPlan {
            required: true,
            commands: vec![
                command("cargo test -p zhongshu-core"),
                command("cargo test -p zhongshu-cli"),
            ],
            environment_notes: vec!["core fallback available".into()],
            fallback_commands: vec![command("cargo check -p zhongshu-core")],
        };
        let mut state = state();
        let mut runner = MockRunner {
            outputs: vec![
                VerificationCommandOutput::failure(1, "compile failed"),
                VerificationCommandOutput::success("check ok"),
            ],
            seen: Vec::new(),
        };

        let report = execute_plan(&mut state, &plan, &mut runner, 20)
            .await
            .unwrap();

        assert!(report.passed);
        assert_eq!(report.command_results.len(), 1);
        assert_eq!(report.fallback_results.len(), 1);
        assert_eq!(
            runner.seen,
            vec![
                "cargo test -p zhongshu-core",
                "cargo check -p zhongshu-core"
            ]
        );
        assert_eq!(state.last_failure.as_ref().map(|r| r.step), Some(21));
        assert_eq!(state.last_success.as_ref().map(|r| r.step), Some(22));
    }

    #[tokio::test]
    async fn reports_blocked_when_primary_and_fallback_fail() {
        let plan = VerificationPlan {
            required: true,
            commands: vec![command("cargo test -p zhongshu-core")],
            environment_notes: Vec::new(),
            fallback_commands: vec![command("cargo check -p zhongshu-core")],
        };
        let mut state = state();
        let mut runner = MockRunner {
            outputs: vec![
                VerificationCommandOutput::failure(1, "test failed"),
                VerificationCommandOutput::failure(1, "check failed"),
            ],
            seen: Vec::new(),
        };

        let report = execute_plan(&mut state, &plan, &mut runner, 30)
            .await
            .unwrap();

        assert!(!report.passed);
        assert!(report.blocked);
        assert!(report
            .failure_summary
            .as_deref()
            .unwrap()
            .contains("test failed"));
        assert_eq!(state.last_failure.as_ref().map(|r| r.step), Some(32));
    }

    #[tokio::test]
    async fn records_unavailable_reason() {
        let plan = VerificationPlan {
            required: true,
            commands: vec![command("cargo check -p zhongshu-orb")],
            environment_notes: Vec::new(),
            fallback_commands: Vec::new(),
        };
        let mut state = state();
        let mut runner = MockRunner {
            outputs: vec![VerificationCommandOutput::unavailable("pkg-config missing")],
            seen: Vec::new(),
        };

        let report = execute_plan(&mut state, &plan, &mut runner, 5)
            .await
            .unwrap();

        assert!(report.blocked);
        assert_eq!(
            state.unavailable_reason.as_deref(),
            Some("pkg-config missing")
        );
        assert_eq!(state.last_failure.as_ref().map(|r| r.exit_code), Some(None));
    }

    #[tokio::test]
    async fn skips_non_required_plan() {
        let plan = VerificationPlan::empty();
        let mut state = state();
        let mut runner = MockRunner::default();

        let report = execute_plan(&mut state, &plan, &mut runner, 1)
            .await
            .unwrap();

        assert!(report.passed);
        assert!(report.command_results.is_empty());
        assert!(state.records.is_empty());
    }

    #[tokio::test]
    async fn shell_runner_executes_verification_command() {
        let mut runner = ShellVerificationRunner::new(std::env::current_dir().unwrap());
        let output = runner.run(&command("cargo check --help")).await.unwrap();

        assert_eq!(output.exit_code, Some(0));
        assert!(!output.stdout.is_empty() || !output.stderr.is_empty());
    }

    #[tokio::test]
    async fn shell_runner_rejects_non_verification_command() {
        let mut runner = ShellVerificationRunner::new(std::env::current_dir().unwrap());
        let output = runner
            .run(&VerificationCommand::new(
                "echo not verification",
                VerificationReason::UserProvided,
            ))
            .await
            .unwrap();

        assert!(output.error.unwrap().contains("not classified"));
    }
}
