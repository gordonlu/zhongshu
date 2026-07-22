use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::agent::llm::{ChatCompletionRequest, Message};
use crate::agent::{
    AgentProfile, AgentRuntime, CollaborationMode, EmployeeCapability, EmployeeRole,
    RoleRequirement, StaffingRequest, DEFAULT_MAX_WORKERS_PER_TASK,
};

const MIN_SEPARABILITY: u8 = 60;
const MIN_SPECIALIZATION_BENEFIT: u8 = 70;
const MAX_MERGE_RISK: u8 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoDelegationStrategy {
    SingleAgent,
    MultiAgent,
}

impl AutoDelegationStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SingleAgent => "single_agent",
            Self::MultiAgent => "multi_agent",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoDelegationDecision {
    pub strategy: AutoDelegationStrategy,
    pub reason: String,
    pub collaboration: CollaborationMode,
    pub staffing: StaffingRequest,
}

impl AutoDelegationDecision {
    pub fn single(objective: impl Into<String>, reason: impl Into<String>) -> Self {
        let objective = objective.into();
        Self {
            strategy: AutoDelegationStrategy::SingleAgent,
            reason: reason.into(),
            collaboration: CollaborationMode::Independent,
            staffing: StaffingRequest::direct(objective),
        }
    }

    pub fn worker_count(&self) -> usize {
        self.staffing.requirements.len()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlannerProposal {
    strategy: AutoDelegationStrategy,
    reason: String,
    mutation_required: bool,
    collaboration: CollaborationMode,
    separability: u8,
    specialization_benefit: u8,
    merge_risk: u8,
    max_workers: usize,
    #[serde(default)]
    requirements: Vec<PlannerRequirement>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlannerRequirement {
    employee: String,
    role: String,
    #[serde(default)]
    capabilities: Vec<String>,
    responsibility: String,
}

pub struct AutoDelegationPlanner;

impl AutoDelegationPlanner {
    /// Ask the current model for a bounded staffing proposal, then admit it
    /// through deterministic policy. Provider and parse failures are visible
    /// in the single-agent fallback reason.
    pub async fn decide(
        runtime: &AgentRuntime,
        objective: &str,
        eligible_roster: &[AgentProfile],
    ) -> AutoDelegationDecision {
        if eligible_roster.len() < 2 {
            return AutoDelegationDecision::single(
                objective,
                "可用于自动只读协作的员工不足 2 名，交由主 AI 处理",
            );
        }

        let roster = eligible_roster
            .iter()
            .map(|profile| {
                serde_json::json!({
                    "employee": profile.name,
                    "role": profile.specialty.role.as_str(),
                    "capabilities": profile.specialty.capabilities.iter().map(|capability| capability.as_str()).collect::<Vec<_>>(),
                    "focus": profile.specialty.focus,
                })
            })
            .collect::<Vec<_>>();
        let roster_json = serde_json::to_string(&roster).unwrap_or_else(|_| "[]".into());
        let system = format!(
            r#"你是多 Agent 路由规划器，只提出候选方案，不执行任务。
仅当任务能拆成 2 到 3 个边界清楚、确有专业收益的只读子任务时选择 multi_agent；否则选择 single_agent。
任何写文件、改代码、发消息、提交表单、操作外部系统或其他副作用，都必须令 mutation_required=true，并选择 single_agent。
员工只能从给定名单精确选择；role 和 capabilities 必须逐字来自该员工资料。员工不得重复。
separability、specialization_benefit、merge_risk 均为 0 到 100 的整数。
只输出一个 JSON 对象，不要 Markdown，不要解释。格式：
{{"strategy":"single_agent|multi_agent","reason":"简短中文理由","mutation_required":false,"collaboration":"independent|sequential_handoff","separability":0,"specialization_benefit":0,"merge_risk":0,"max_workers":0,"requirements":[{{"employee":"精确名称","role":"精确岗位","capabilities":["精确能力"],"responsibility":"清晰且不重叠的职责"}}]}}
可用员工：{roster_json}"#
        );
        let request = ChatCompletionRequest {
            model: runtime.model.clone(),
            messages: vec![Message::system(system), Message::user(objective)],
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: Some(0.0),
            max_tokens: Some(1200),
            reasoning_effort: runtime.reasoning_effort.clone(),
        };

        let response = match runtime.provider.chat(request).await {
            Ok(response) => response,
            Err(error) => {
                return AutoDelegationDecision::single(
                    objective,
                    format!("自动编排规划失败，已回退主 AI：{error}"),
                );
            }
        };
        let Some(choice) = response.choices.into_iter().next() else {
            return AutoDelegationDecision::single(
                objective,
                "自动编排规划未返回候选方案，已回退主 AI",
            );
        };
        Self::decide_from_text(objective, eligible_roster, &choice.message.content)
    }

    fn decide_from_text(
        objective: &str,
        eligible_roster: &[AgentProfile],
        text: &str,
    ) -> AutoDelegationDecision {
        let json = extract_json_object(text).unwrap_or(text.trim());
        let proposal = match serde_json::from_str::<PlannerProposal>(json) {
            Ok(proposal) => proposal,
            Err(error) => {
                return AutoDelegationDecision::single(
                    objective,
                    format!("自动编排候选方案格式无效，已回退主 AI：{error}"),
                );
            }
        };
        admit_proposal(objective, eligible_roster, proposal)
    }
}

fn admit_proposal(
    objective: &str,
    roster: &[AgentProfile],
    proposal: PlannerProposal,
) -> AutoDelegationDecision {
    if proposal.strategy == AutoDelegationStrategy::SingleAgent {
        let reason = if proposal.reason.trim().is_empty() {
            "规划器判断多 Agent 收益不足".to_string()
        } else {
            proposal.reason.trim().to_string()
        };
        return AutoDelegationDecision::single(objective, reason);
    }
    if proposal.mutation_required {
        return AutoDelegationDecision::single(
            objective,
            "任务涉及副作用；自动模式没有用户定义的文件或资源边界，交由主 AI 处理",
        );
    }
    if proposal.separability > 100
        || proposal.specialization_benefit > 100
        || proposal.merge_risk > 100
    {
        return AutoDelegationDecision::single(objective, "自动编排评分超出 0–100，已回退主 AI");
    }
    if proposal.separability < MIN_SEPARABILITY
        && proposal.specialization_benefit < MIN_SPECIALIZATION_BENEFIT
    {
        return AutoDelegationDecision::single(
            objective,
            "子任务可分性和专业化收益未达到自动多 Agent 准入线",
        );
    }
    if proposal.merge_risk > MAX_MERGE_RISK {
        return AutoDelegationDecision::single(objective, "结果合并风险过高，交由主 AI 统一处理");
    }

    let count = proposal.requirements.len();
    if !(2..=DEFAULT_MAX_WORKERS_PER_TASK).contains(&count)
        || proposal.max_workers < count
        || proposal.max_workers > DEFAULT_MAX_WORKERS_PER_TASK
    {
        return AutoDelegationDecision::single(
            objective,
            format!(
                "自动多 Agent 方案必须使用 2–{DEFAULT_MAX_WORKERS_PER_TASK} 名员工，已回退主 AI"
            ),
        );
    }

    let mut employees = HashSet::new();
    let mut requirements = Vec::with_capacity(count);
    for requirement in proposal.requirements {
        let employee = requirement.employee.trim();
        if employee.is_empty() || !employees.insert(employee.to_string()) {
            return AutoDelegationDecision::single(
                objective,
                "自动编排包含空或重复员工，已回退主 AI",
            );
        }
        let Some(profile) = roster.iter().find(|profile| profile.name == employee) else {
            return AutoDelegationDecision::single(
                objective,
                "自动编排选择了名单外员工，已回退主 AI",
            );
        };
        if requirement.role.trim() != profile.specialty.role.as_str() {
            return AutoDelegationDecision::single(
                objective,
                "自动编排的员工岗位不匹配，已回退主 AI",
            );
        }
        if requirement.responsibility.trim().is_empty() {
            return AutoDelegationDecision::single(objective, "自动编排存在空职责，已回退主 AI");
        }
        let capabilities = requirement
            .capabilities
            .iter()
            .map(|capability| EmployeeCapability::new(capability))
            .collect::<Vec<_>>();
        if capabilities.iter().any(|capability| {
            !capability.is_valid() || !profile.specialty.capabilities.contains(capability)
        }) {
            return AutoDelegationDecision::single(
                objective,
                "自动编排要求了员工不具备的能力，已回退主 AI",
            );
        }
        requirements.push(RoleRequirement {
            role: EmployeeRole::new(requirement.role),
            employee: Some(employee.to_string()),
            capabilities,
            responsibility: requirement.responsibility.trim().to_string(),
            required: true,
        });
    }

    AutoDelegationDecision {
        strategy: AutoDelegationStrategy::MultiAgent,
        reason: format!(
            "{}（可分性 {}，专业收益 {}，合并风险 {}）",
            proposal.reason.trim(),
            proposal.separability,
            proposal.specialization_benefit,
            proposal.merge_risk
        ),
        collaboration: proposal.collaboration,
        staffing: StaffingRequest {
            objective: objective.to_string(),
            requirements,
            max_workers: Some(count),
        },
    }
}

fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end >= start).then_some(&text[start..=end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Arc;

    use crate::agent::llm::{ChatCompletionResponse, FinalChoice, LlmProvider, StreamEvent};
    use crate::agent::loop_::AgentBudget;
    use crate::tool::ToolRegistry;

    #[derive(Clone)]
    struct ProposalProvider;

    #[async_trait]
    impl LlmProvider for ProposalProvider {
        async fn chat(
            &self,
            request: ChatCompletionRequest,
        ) -> anyhow::Result<ChatCompletionResponse> {
            assert!(!request.stream);
            assert!(request.tools.is_none());
            assert_eq!(request.temperature, Some(0.0));
            assert_eq!(request.max_tokens, Some(1200));
            Ok(ChatCompletionResponse {
                choices: vec![FinalChoice {
                    message: Message::assistant(
                        r#"{"strategy":"multi_agent","reason":"研究与成稿可分离","mutation_required":false,"collaboration":"sequential_handoff","separability":85,"specialization_benefit":90,"merge_risk":25,"max_workers":2,"requirements":[{"employee":"researcher","role":"research","capabilities":["source_review"],"responsibility":"整理来源"},{"employee":"writer","role":"writing","capabilities":["synthesis"],"responsibility":"综合成稿"}]}"#,
                    ),
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
            anyhow::bail!("planner must not use streaming")
        }

        fn model_name(&self) -> &str {
            "planner-test"
        }

        fn change_model(&self, _model: &str) -> Arc<dyn LlmProvider> {
            Arc::new(self.clone())
        }
    }

    fn profile(name: &str, role: &str, capabilities: &[&str]) -> AgentProfile {
        AgentProfile::new(
            name,
            "test",
            vec!["read_file".into()],
            AgentBudget::default(),
        )
        .with_specialty(
            EmployeeRole::new(role),
            capabilities
                .iter()
                .map(|capability| EmployeeCapability::new(*capability))
                .collect(),
            "test focus",
        )
    }

    fn roster() -> Vec<AgentProfile> {
        vec![
            profile("researcher", "research", &["source_review"]),
            profile("writer", "writing", &["synthesis"]),
        ]
    }

    #[tokio::test]
    async fn requests_one_non_streaming_proposal_before_deterministic_admission() {
        let runtime = AgentRuntime::new(
            ProposalProvider,
            ToolRegistry::new(),
            "planner-test",
            AgentBudget::default(),
        );
        let decision = AutoDelegationPlanner::decide(&runtime, "prepare a report", &roster()).await;

        assert_eq!(decision.strategy, AutoDelegationStrategy::MultiAgent);
        assert_eq!(decision.worker_count(), 2);
    }

    #[test]
    fn admits_bounded_exact_roster_proposal() {
        let decision = AutoDelegationPlanner::decide_from_text(
            "prepare a report",
            &roster(),
            r#"```json
            {"strategy":"multi_agent","reason":"研究与成稿可分离","mutation_required":false,"collaboration":"sequential_handoff","separability":85,"specialization_benefit":90,"merge_risk":25,"max_workers":2,"requirements":[{"employee":"researcher","role":"research","capabilities":["source_review"],"responsibility":"整理来源"},{"employee":"writer","role":"writing","capabilities":["synthesis"],"responsibility":"综合成稿"}]}
            ```"#,
        );
        assert_eq!(decision.strategy, AutoDelegationStrategy::MultiAgent);
        assert_eq!(decision.worker_count(), 2);
        assert_eq!(decision.collaboration, CollaborationMode::SequentialHandoff);
    }

    #[test]
    fn mutation_proposal_falls_back_to_single_agent() {
        let decision = AutoDelegationPlanner::decide_from_text(
            "edit files",
            &roster(),
            r#"{"strategy":"multi_agent","reason":"split","mutation_required":true,"collaboration":"independent","separability":90,"specialization_benefit":90,"merge_risk":10,"max_workers":2,"requirements":[]}"#,
        );
        assert_eq!(decision.strategy, AutoDelegationStrategy::SingleAgent);
        assert!(decision.reason.contains("副作用"));
    }

    #[test]
    fn invented_capability_falls_back_to_single_agent() {
        let decision = AutoDelegationPlanner::decide_from_text(
            "prepare a report",
            &roster(),
            r#"{"strategy":"multi_agent","reason":"split","mutation_required":false,"collaboration":"independent","separability":90,"specialization_benefit":90,"merge_risk":10,"max_workers":2,"requirements":[{"employee":"researcher","role":"research","capabilities":["invented"],"responsibility":"research"},{"employee":"writer","role":"writing","capabilities":["synthesis"],"responsibility":"write"}]}"#,
        );
        assert_eq!(decision.strategy, AutoDelegationStrategy::SingleAgent);
        assert!(decision.reason.contains("不具备"));
    }

    #[test]
    fn high_merge_risk_falls_back_to_single_agent() {
        let decision = AutoDelegationPlanner::decide_from_text(
            "prepare a report",
            &roster(),
            r#"{"strategy":"multi_agent","reason":"split","mutation_required":false,"collaboration":"independent","separability":90,"specialization_benefit":90,"merge_risk":80,"max_workers":2,"requirements":[]}"#,
        );
        assert_eq!(decision.strategy, AutoDelegationStrategy::SingleAgent);
        assert!(decision.reason.contains("合并风险"));
    }
}
