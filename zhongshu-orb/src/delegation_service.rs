use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::RwLock;
use zhongshu_core::agent::llm_registry::{LlmClient, LlmRegistry};
use zhongshu_core::agent::run::RunController;
use zhongshu_core::agent::{AgentProfile, AgentRuntime, Orchestrator, WorkerExecutionStatus};
use zhongshu_core::event::{
    AgentEvent, AgentState, Event, EventBus, HarnessUiEvent, MessageId, ResponseEvent,
    ResponseRole, ResponseTx,
};

use crate::app::publish_harness_events;

/// Production entrypoint for the dedicated bounded review pipeline:
/// one low-cost analyst hands a report to one low-cost verifier, then the
/// primary model summarizes while deterministic evidence rules decide whether
/// the Lead may accept the result.
pub struct DelegationController {
    runtime: Arc<RwLock<AgentRuntime>>,
    analyst_profile: AgentProfile,
    verifier_profile: AgentProfile,
    llm_registry: Arc<LlmRegistry>,
    event_bus: Arc<EventBus>,
    response_tx: ResponseTx,
    run_controller: Arc<RunController>,
    busy: Arc<AtomicBool>,
    current_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    current_session: Arc<Mutex<Option<String>>>,
}

impl DelegationController {
    pub fn new(
        runtime: Arc<RwLock<AgentRuntime>>,
        analyst_profile: AgentProfile,
        verifier_profile: AgentProfile,
        llm_registry: Arc<LlmRegistry>,
        event_bus: Arc<EventBus>,
        response_tx: ResponseTx,
        run_controller: Arc<RunController>,
    ) -> Self {
        Self {
            runtime,
            analyst_profile,
            verifier_profile,
            llm_registry,
            event_bus,
            response_tx,
            run_controller,
            busy: Arc::new(AtomicBool::new(false)),
            current_task: Arc::new(Mutex::new(None)),
            current_session: Arc::new(Mutex::new(None)),
        }
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    pub fn submit_review(&self, goal: String) -> bool {
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }

        let runtime = self.runtime.clone();
        let analyst_profile = self.analyst_profile.clone();
        let verifier_profile = self.verifier_profile.clone();
        let llm_registry = self.llm_registry.clone();
        let event_bus = self.event_bus.clone();
        let response_tx = self.response_tx.clone();
        let busy = self.busy.clone();
        let current_task = self.current_task.clone();
        let current_session = self.current_session.clone();
        let session_id = format!("delegation-{}", uuid::Uuid::new_v4());
        *current_session.lock().unwrap() = Some(session_id.clone());
        let run_id = self.run_controller.start_run(&goal);
        let run_controller = self.run_controller.clone();
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();

        event_bus.publish(Event::Harness(HarnessUiEvent::CodingPlanCreated {
            session_id: session_id.clone(),
            step_count: 2,
            risk: "review-only".into(),
        }));
        event_bus.publish(Event::Agent(AgentEvent::StateChanged {
            from: AgentState::Idle,
            to: AgentState::Thinking,
        }));

        let handle = tokio::spawn(async move {
            let _ = start_rx.await;
            let runtime = runtime.read().await.clone();
            let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
            let organization_bus = event_bus.clone();
            let result = orchestrator
                .execute_review_pipeline_with_events(
                    &goal,
                    analyst_profile,
                    verifier_profile,
                    &session_id,
                    move |event| organization_bus.publish(Event::Organization(event)),
                )
                .await;

            let (message, final_state) = match result {
                Ok(report) => {
                    publish_harness_events(&event_bus, &report.trace_events);
                    let lead_summary = match llm_registry.client_for_role("primary") {
                        Ok(client) => lead_summary(&orchestrator, &goal, &report, &client).await,
                        Err(error) => {
                            tracing::warn!(%error, "primary Lead model unavailable for delegation summary");
                            deterministic_summary(&report)
                        }
                    };
                    match report.status {
                        WorkerExecutionStatus::Completed => (
                            format!("中书验收通过。\n\n{lead_summary}"),
                            AgentState::Done { success: true },
                        ),
                        WorkerExecutionStatus::Submitted => (
                            format!(
                                "两名员工已提交报告，但中书尚未验收：缺少新鲜、通过的验证证据。\n\n{lead_summary}"
                            ),
                            AgentState::Submitted,
                        ),
                        WorkerExecutionStatus::BlockedBeforeExecution
                        | WorkerExecutionStatus::WorkerFailed
                        | WorkerExecutionStatus::CompletedWithReviewFindings => (
                            format!("员工协作未通过中书验收。\n\n{lead_summary}"),
                            AgentState::Done { success: false },
                        ),
                    }
                }
                Err(error) => (
                    format!("双员工协作执行失败：{error}"),
                    AgentState::Done { success: false },
                ),
            };

            emit_assistant_message(&response_tx, run_id, &message).await;
            event_bus.publish(Event::Agent(AgentEvent::StateChanged {
                from: AgentState::Thinking,
                to: final_state,
            }));
            event_bus.publish(Event::Agent(AgentEvent::StateChanged {
                from: final_state,
                to: AgentState::Idle,
            }));
            run_controller
                .finish_run(
                    match final_state {
                        AgentState::Done { success: true } => "completed_verified",
                        AgentState::Submitted => "completed_unverified",
                        _ => "failed",
                    },
                    None,
                )
                .await;
            current_task.lock().unwrap().take();
            current_session.lock().unwrap().take();
            // Clear the stored handle before reopening admission. Otherwise a
            // new submission can install its handle in the small window
            // between these operations and this completed task would remove it.
            busy.store(false, Ordering::Release);
        });
        *self.current_task.lock().unwrap() = Some(handle);
        let _ = start_tx.send(());
        true
    }

    pub fn cancel(&self) -> bool {
        let Some(handle) = self.current_task.lock().unwrap().take() else {
            return false;
        };
        handle.abort();
        if let Some(task_id) = self.current_session.lock().unwrap().take() {
            self.event_bus.publish(Event::Organization(
                zhongshu_core::event::OrganizationEvent::TaskFinished {
                    task_id,
                    status: "cancelled".into(),
                    reason: Some("cancelled by user".into()),
                },
            ));
        }
        self.busy.store(false, Ordering::Release);
        let run_controller = self.run_controller.clone();
        tokio::spawn(async move {
            run_controller.finish_run("cancelled", None).await;
        });
        self.event_bus
            .publish(Event::Agent(AgentEvent::StateChanged {
                from: AgentState::Thinking,
                to: AgentState::Done { success: false },
            }));
        self.event_bus
            .publish(Event::Agent(AgentEvent::StateChanged {
                from: AgentState::Done { success: false },
                to: AgentState::Idle,
            }));
        true
    }
}

async fn lead_summary(
    orchestrator: &Orchestrator,
    goal: &str,
    report: &zhongshu_core::agent::LeadReviewReport,
    client: &LlmClient,
) -> String {
    let reports = [report.analyst.clone(), report.verifier.clone()];
    match orchestrator
        .parent_review(goal, &reports, &[], client)
        .await
    {
        Ok(review) if !review.findings.trim().is_empty() => review.findings,
        Ok(_) => deterministic_summary(report),
        Err(error) => {
            tracing::warn!(%error, "Lead summary failed; using deterministic report");
            deterministic_summary(report)
        }
    }
}

fn deterministic_summary(report: &zhongshu_core::agent::LeadReviewReport) -> String {
    let mut text = format!(
        "分析员工：{}\n\n验证员工：{}",
        report.analyst.summary, report.verifier.summary
    );
    if !report.acceptance_reasons.is_empty() {
        text.push_str("\n\n未通过原因：");
        for reason in &report.acceptance_reasons {
            text.push_str("\n- ");
            text.push_str(reason);
        }
    }
    text
}

pub(crate) async fn emit_assistant_message(
    response_tx: &ResponseTx,
    run_id: uuid::Uuid,
    message: &str,
) {
    let id = MessageId::new();
    if response_tx
        .send(ResponseEvent::MessageStarted {
            id,
            role: ResponseRole::Assistant,
            run_id,
        })
        .await
        .is_err()
    {
        return;
    }
    if response_tx
        .send(ResponseEvent::MessageDelta {
            id,
            delta: message.to_string(),
            run_id,
        })
        .await
        .is_err()
    {
        return;
    }
    let _ = response_tx
        .send(ResponseEvent::MessageCompleted { id, run_id })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use zhongshu_core::agent::{AttentionLevel, LeadReviewReport, Report, RunOutcome};

    fn report(worker: &str, outcome: RunOutcome, summary: &str) -> Report {
        Report {
            task_id: worker.into(),
            worker: worker.into(),
            run_id: "unknown".into(),
            summary: summary.into(),
            findings: summary.into(),
            success: outcome == RunOutcome::CompletedVerified,
            outcome,
            confidence: 0.5,
            attention: AttentionLevel::Digest,
            trace_events: Vec::new(),
        }
    }

    #[test]
    fn deterministic_summary_keeps_acceptance_reasons_visible() {
        let report = LeadReviewReport {
            status: WorkerExecutionStatus::Submitted,
            recovery: zhongshu_core::agent::ReviewPipelineRecovery::NotNeeded,
            analyst: report("analyst", RunOutcome::CompletedUnverified, "found issue"),
            verifier: report("verifier", RunOutcome::CompletedUnverified, "no tests"),
            acceptance_reasons: vec!["missing verification".into()],
            trace_events: Vec::new(),
        };

        let summary = deterministic_summary(&report);
        assert!(summary.contains("found issue"));
        assert!(summary.contains("missing verification"));
    }
}
