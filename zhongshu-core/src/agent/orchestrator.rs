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

/// A file-scoped sub-task assignment for a single worker.
pub struct WorkerAssignment {
    pub worker_name: String,
    pub task_description: String,
    pub owned_files: Vec<PathBuf>,
    pub profile: AgentProfile,
}

/// A file edit conflict between two workers.
pub struct Conflict {
    pub file: PathBuf,
    pub workers: Vec<String>,
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
}

impl Orchestrator {
    pub fn new(runtime: AgentRuntime, registry: LlmRegistry) -> Self {
        Orchestrator { runtime, registry }
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
    pub async fn execute(
        &self,
        assignments: Vec<WorkerAssignment>,
    ) -> anyhow::Result<Vec<Report>> {
        let mut reports = Vec::new();

        for a in &assignments {
            let task = crate::task::Task {
                id: format!("worker-{}", a.worker_name),
                source: "orchestrator".into(),
                tool: "agent".into(),
                arguments: serde_json::json!({
                    "task": a.task_description,
                }),
            };

            let report = Worker::execute(&self.runtime, &a.profile, task, None).await?;
            reports.push(report);
        }

        Ok(reports)
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
            confidence: 0.7,
            attention: AttentionLevel::Digest,
            trace_events: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::agent::llm::{ChatCompletionResponse, FinalChoice, LlmProvider};
    use crate::agent::AgentBudget;
    use crate::harness::architecture::index::FileIndex;
    use crate::tool::ToolRegistry;
    use std::sync::Arc;

    struct MockProvider;

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

    fn dummy_profile(name: &str) -> AgentProfile {
        AgentProfile::new(
            name,
            "你是一个测试 worker。",
            vec![],
            AgentBudget::default(),
        )
    }

    fn dummy_runtime() -> AgentRuntime {
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
            attention: AttentionLevel::Digest,
            trace_events: vec![HarnessEvent::FileEdit {
                path: PathBuf::from("a.rs"),
                diff_hash: "abc".into(),
            }],
        };
        let report_b = Report {
            task_id: "t2".into(),
            worker: "w2".into(),
            summary: "".into(),
            findings: "".into(),
            confidence: 0.5,
            attention: AttentionLevel::Digest,
            trace_events: vec![HarnessEvent::FileEdit {
                path: PathBuf::from("b.rs"),
                diff_hash: "def".into(),
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
            attention: AttentionLevel::Digest,
            trace_events: vec![HarnessEvent::FileEdit {
                path: PathBuf::from("shared.rs"),
                diff_hash: "abc".into(),
            }],
        };
        let report_b = Report {
            task_id: "t2".into(),
            worker: "w2".into(),
            summary: "".into(),
            findings: "".into(),
            confidence: 0.5,
            attention: AttentionLevel::Digest,
            trace_events: vec![HarnessEvent::FileEdit {
                path: PathBuf::from("shared.rs"),
                diff_hash: "def".into(),
            }],
        };

        let orch = Orchestrator::new(dummy_runtime(), LlmRegistry::new());
        let conflicts = orch.detect_conflicts(&[report_a, report_b]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].file, PathBuf::from("shared.rs"));
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
            .parent_review(
                "添加 login 功能",
                &[],
                &[],
                &client,
            )
            .await
            .expect("parent review should succeed");

        assert!(report.findings.contains("一切正常"));
        assert_eq!(report.worker, "orchestrator");
    }
}
