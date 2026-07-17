//! Attack-case tests for file editing safety in the DS Coding Harness.
//!
//! Each test simulates an adversarial agent pattern and verifies that
//! the harness catches it.
//!
//! Guards under test:
//! - PatchEngine read-before-write (FileNotRead error)
//! - PatchEngine stale-read detection (StaleRead error)
//! - Orchestrator ownership violation detection (OwnershipViolation)

use std::path::PathBuf;
use std::sync::Arc;

use zhongshu_core::agent::llm::{
    ChatCompletionRequest, ChatCompletionResponse, FinalChoice, LlmProvider, Message, StreamEvent,
};
use zhongshu_core::agent::llm_registry::LlmRegistry;
use zhongshu_core::agent::loop_::AgentBudget;
use zhongshu_core::agent::orchestrator::{Orchestrator, WorkerAssignment};
use zhongshu_core::agent::report::Report;
use zhongshu_core::agent::runtime::AgentRuntime;
use zhongshu_core::agent::AttentionLevel;
use zhongshu_core::harness::trace::event::HarnessEvent;
use zhongshu_core::patch::{PatchEngine, PatchError, PatchOperation, ReplaceRequest};
use zhongshu_core::tool::ToolRegistry;

// ═══════════════════════════════════════════════════════════════════════
// Mock LLM provider (required by Orchestrator, never called in our test)
// ═══════════════════════════════════════════════════════════════════════

struct MockProvider;

#[async_trait::async_trait]
impl LlmProvider for MockProvider {
    async fn chat(
        &self,
        _request: ChatCompletionRequest,
    ) -> anyhow::Result<ChatCompletionResponse> {
        Ok(ChatCompletionResponse {
            choices: vec![FinalChoice {
                message: Message::assistant("ok"),
                finish_reason: Some("stop".into()),
            }],
            usage: None,
        })
    }

    async fn stream_chat(
        &self,
        _request: ChatCompletionRequest,
        _on_event: Box<dyn FnMut(StreamEvent) + Send>,
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

fn dummy_orchestrator() -> Orchestrator {
    let runtime = AgentRuntime::new(
        MockProvider,
        ToolRegistry::new(),
        "mock",
        AgentBudget::assistant_default(),
    );
    Orchestrator::new(runtime, LlmRegistry::new())
}

// ═══════════════════════════════════════════════════════════════════════
// Guard 1: Edit without prior read
//
// PatchEngine enforces read-before-write: every patch operation on an
// existing file requires a prior read() call.  If the file was never
// read, PatchEngine returns FileNotRead.
//
// NOTE: There is currently no higher-level check in the harness that
// enforces tool-call ordering (e.g. "edit" must follow "read_file").
// This test validates the guard at the patch-engine level.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn edit_without_read_is_blocked() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello").expect("write test file");

    let mut engine = PatchEngine::new(dir.path()).expect("PatchEngine::new");

    let err = engine
        .apply_operation(PatchOperation::Replace(ReplaceRequest::once(
            "a.txt", "hello", "hi",
        )))
        .unwrap_err();

    assert!(
        matches!(err.error, PatchError::FileNotRead { .. }),
        "edit without prior read must produce FileNotRead, got: {err:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Guard 2: Stale-read patch
//
// Worker reads file at version A, the file changes to version B
// (e.g. by another process or concurrent worker), then the worker
// attempts to apply a patch based on version A.  PatchEngine catches
// the mismatch and returns StaleRead.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn stale_read_patch_blocked() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("a.txt");

    // Write version A
    std::fs::write(&file, "version A").expect("write version A");

    let mut engine = PatchEngine::new(dir.path()).expect("PatchEngine::new");

    // Worker reads at version A
    let snapshot = engine.read("a.txt").expect("read should succeed");
    assert_eq!(snapshot.content, "version A");

    // External change: file becomes version B (bypassing PatchEngine)
    std::fs::write(&file, "version B").expect("write version B");

    // Worker tries to apply patch based on version A
    let err = engine
        .apply_operation(PatchOperation::Replace(ReplaceRequest::once(
            "a.txt",
            "version A",
            "version C",
        )))
        .unwrap_err();

    assert!(
        matches!(err.error, PatchError::StaleRead { .. }),
        "stale read after external modification must produce StaleRead, got: {err:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Guard 3: Worker patch outside owned files
//
// A worker is assigned owned_files = ["src/a.rs"] but attempts to edit
// "src/b.rs".  The orchestrator's detect_ownership_violations must
// return the violation.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn worker_patch_outside_owned_files_detected() {
    let assignment = WorkerAssignment {
        worker_name: "w1".into(),
        task_description: "edit owned files".into(),
        owned_files: vec![PathBuf::from("src/a.rs")],
        profile: zhongshu_core::agent::profile::AgentProfile::new(
            "w1",
            "test worker",
            vec![],
            AgentBudget::assistant_default(),
        ),
    };

    let report = Report {
        task_id: "t1".into(),
        worker: "w1".into(),
        summary: "".into(),
        findings: "".into(),
        confidence: 0.5,
        success: true,
        outcome: zhongshu_core::agent::RunOutcome::CompletedVerified,
        attention: AttentionLevel::Digest,
        trace_events: vec![HarnessEvent::FileEdit {
            path: PathBuf::from("src/b.rs"),
            diff_hash: "abc".into(),
            diff: None,
        }],
    };

    let orch = dummy_orchestrator();
    let violations = orch.detect_ownership_violations(&[assignment], &[report]);

    assert_eq!(violations.len(), 1, "must detect the violation");
    assert_eq!(violations[0].worker, "w1");
    assert_eq!(violations[0].file, PathBuf::from("src/b.rs"));
    assert!(
        violations[0].reason.contains("outside"),
        "reason must mention 'outside', got: {}",
        violations[0].reason
    );
}

#[test]
fn worker_patch_inside_owned_files_allowed() {
    let assignment = WorkerAssignment {
        worker_name: "w1".into(),
        task_description: "edit owned files".into(),
        owned_files: vec![PathBuf::from("src/a.rs")],
        profile: zhongshu_core::agent::profile::AgentProfile::new(
            "w1",
            "test worker",
            vec![],
            AgentBudget::assistant_default(),
        ),
    };

    let report = Report {
        task_id: "t1".into(),
        worker: "w1".into(),
        summary: "".into(),
        findings: "".into(),
        confidence: 0.5,
        success: true,
        outcome: zhongshu_core::agent::RunOutcome::CompletedVerified,
        attention: AttentionLevel::Digest,
        trace_events: vec![HarnessEvent::FileEdit {
            path: PathBuf::from("src/a.rs"),
            diff_hash: "abc".into(),
            diff: None,
        }],
    };

    let orch = dummy_orchestrator();
    let violations = orch.detect_ownership_violations(&[assignment], &[report]);

    assert!(
        violations.is_empty(),
        "edit inside owned files must not produce violations"
    );
}
