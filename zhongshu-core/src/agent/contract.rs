use std::path::PathBuf;

use crate::agent::attention::AttentionLevel;
use crate::agent::loop_::{AgentBudget, RunOutcome};
use crate::agent::profile::AgentProfile;
use crate::agent::report::Report;
use crate::harness::trace::event::HarnessEvent;
use crate::task::Task;

// ── DelegationContract ────────────────────────────────────────────────

/// 一次委派的完整结构化契约。
///
/// Lead Agent 用此契约约束一个 Worker 的范围、预算、权限和交付标准。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DelegationContract {
    /// Worker 名称（对应 profile.name）。
    pub worker: String,
    /// 任务描述（自由文本，由 LLM 消费）。
    pub task_description: String,
    /// 工作范围（文件、目录）。
    pub scope: WorkScope,
    /// 资源预算。
    pub budget: DelegationBudget,
    /// 工具权限。
    pub permissions: DelegationPermissions,
    /// 验收标准。
    pub acceptance: AcceptanceCriteria,
    /// 期望的产出物。
    pub artifacts: ArtifactRequirements,
    /// 升级规则。
    pub escalation: EscalationRules,
}

/// 工作范围：Worker 可以读取或修改哪些路径。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkScope {
    /// 归属该 Worker 的文件列表（排他）。
    #[serde(default)]
    pub owned_files: Vec<PathBuf>,
    /// 允许访问的目录（非排他，可与其他 Worker 共享读取）。
    #[serde(default)]
    pub allowed_directories: Vec<PathBuf>,
}

impl WorkScope {
    pub fn new(owned_files: Vec<PathBuf>) -> Self {
        WorkScope {
            owned_files,
            allowed_directories: Vec::new(),
        }
    }
}

/// 委派的资源预算。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DelegationBudget {
    pub max_steps: u32,
    pub max_tool_calls: u32,
    pub token_limit: usize,
    pub timeout_secs: u64,
}

impl Default for DelegationBudget {
    fn default() -> Self {
        let default_budget = AgentBudget::assistant_default();
        DelegationBudget {
            max_steps: default_budget.max_steps,
            max_tool_calls: default_budget.max_tool_calls,
            token_limit: default_budget.token_limit,
            timeout_secs: default_budget.llm_timeout.as_secs(),
        }
    }
}

/// 工具权限。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DelegationPermissions {
    /// 白名单：允许的工具（空 = 允许全部）。
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// 黑名单：禁止的工具。
    #[serde(default)]
    pub denied_tools: Vec<String>,
    /// 是否需要用户审批关键操作。
    #[serde(default)]
    pub require_approval: bool,
}

impl Default for DelegationPermissions {
    fn default() -> Self {
        DelegationPermissions {
            allowed_tools: Vec::new(),
            denied_tools: Vec::new(),
            require_approval: true,
        }
    }
}

/// 验收标准。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AcceptanceCriteria {
    /// 是否需要运行验证命令。
    #[serde(default = "default_true")]
    pub verification_required: bool,
    /// 测试必须通过。
    #[serde(default)]
    pub tests_must_pass: bool,
    /// 不允许越权修改文件。
    #[serde(default = "default_true")]
    pub no_ownership_violations: bool,
    /// 自定义规则（自由文本，由 LLM 判断）。
    #[serde(default)]
    pub custom_rules: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl Default for AcceptanceCriteria {
    fn default() -> Self {
        AcceptanceCriteria {
            verification_required: true,
            tests_must_pass: false,
            no_ownership_violations: true,
            custom_rules: Vec::new(),
        }
    }
}

/// 期望的产出物。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArtifactRequirements {
    /// 是否需要返回 patch。
    #[serde(default = "default_true")]
    pub require_patches: bool,
    /// 是否需要验证证据。
    #[serde(default)]
    pub require_verification_evidence: bool,
    /// 是否需要命令执行日志。
    #[serde(default)]
    pub require_command_log: bool,
}

impl Default for ArtifactRequirements {
    fn default() -> Self {
        ArtifactRequirements {
            require_patches: true,
            require_verification_evidence: false,
            require_command_log: false,
        }
    }
}

/// 升级规则：Worker 在什么情况下应升级给 Lead。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EscalationRules {
    /// 最大重试次数。
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// 失败时是否升级。
    #[serde(default = "default_true")]
    pub escalate_on_failure: bool,
    /// 阻塞时（如 MaxStepsReached）是否升级。
    #[serde(default = "default_true")]
    pub escalate_on_blocked: bool,
}

fn default_max_retries() -> u32 {
    3
}

impl Default for EscalationRules {
    fn default() -> Self {
        EscalationRules {
            max_retries: 3,
            escalate_on_failure: true,
            escalate_on_blocked: true,
        }
    }
}

impl DelegationContract {
    /// 构造一个简单契约。
    pub fn new(worker: impl Into<String>, task_description: impl Into<String>) -> Self {
        DelegationContract {
            worker: worker.into(),
            task_description: task_description.into(),
            scope: WorkScope::new(Vec::new()),
            budget: DelegationBudget::default(),
            permissions: DelegationPermissions::default(),
            acceptance: AcceptanceCriteria::default(),
            artifacts: ArtifactRequirements::default(),
            escalation: EscalationRules::default(),
        }
    }

    /// 设置工作范围。
    pub fn with_scope(mut self, scope: WorkScope) -> Self {
        self.scope = scope;
        self
    }

    /// 设置预算。
    pub fn with_budget(mut self, budget: DelegationBudget) -> Self {
        self.budget = budget;
        self
    }

    /// 设置权限。
    pub fn with_permissions(mut self, permissions: DelegationPermissions) -> Self {
        self.permissions = permissions;
        self
    }
}

impl From<&DelegationContract> for Task {
    fn from(contract: &DelegationContract) -> Self {
        let id = format!("contract-{}-{}", contract.worker, uuid::Uuid::new_v4(),);
        Task {
            id,
            source: "orchestrator".into(),
            tool: "agent".into(),
            arguments: serde_json::json!({"task": contract.task_description}),
        }
    }
}

impl From<&DelegationContract> for AgentProfile {
    fn from(contract: &DelegationContract) -> Self {
        // Filter allowed_tools by removing any that appear in denied_tools.
        let denied: std::collections::HashSet<&str> = contract
            .permissions
            .denied_tools
            .iter()
            .map(String::as_str)
            .collect();
        let effective_tools: Vec<String> = contract
            .permissions
            .allowed_tools
            .iter()
            .filter(|t| !denied.contains(t.as_str()))
            .cloned()
            .collect();

        // Contract acceptance is a parent-side gate: a worker may submit an
        // unverified report for a later verifier. Do not infer runtime
        // verification ownership from `verification_required`; specialized
        // workflows must set AgentProfile.verification_policy explicitly.
        AgentProfile::new(
            &contract.worker,
            format!("你是一个 AI 助手，任务是：{}", contract.task_description),
            effective_tools,
            AgentBudget {
                max_steps: contract.budget.max_steps,
                max_tool_calls: contract.budget.max_tool_calls,
                per_tool_limit: AgentBudget::assistant_default().per_tool_limit,
                token_limit: contract.budget.token_limit,
                llm_timeout: std::time::Duration::from_secs(contract.budget.timeout_secs),
                tool_timeout: AgentBudget::assistant_default().tool_timeout,
            },
        )
    }
}

// ── WorkerOutcome ─────────────────────────────────────────────────────

/// Worker 执行的结构化产出。
///
/// 替代 / 补充 `Report`：除了文本输出外，还携带结构化 artifact、
/// 命令记录和验收证据。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkerOutcome {
    pub worker: String,
    pub status: WorkerStatus,
    pub summary: String,
    pub findings: String,
    pub artifacts: WorkerArtifacts,
    pub attention: AttentionLevel,
    pub confidence: f64,
    pub outcome: RunOutcome,
    pub escalation_reason: Option<String>,
    pub trace_events: Vec<HarnessEvent>,
}

/// Worker 的执行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WorkerStatus {
    /// 按契约要求完成。
    Completed,
    /// Worker 已提交结果，但缺少完成验收所需的验证证据。
    Submitted,
    /// 完成但存在问题（如验证失败但仍提供了产出）。
    CompletedWithIssues,
    /// 被阻塞（预算、步骤、工具调用次数上限）。
    Blocked,
    /// 执行失败。
    Failed,
    /// 被中断。
    Interrupted,
}

impl WorkerStatus {
    pub fn is_success(self) -> bool {
        matches!(self, WorkerStatus::Completed)
    }

    pub fn is_terminal(self) -> bool {
        // All WorkerStatus variants are terminal — the enum represents
        // final outcomes, not intermediate states.
        true
    }
}

/// Worker 执行后产生的结构化产出物。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WorkerArtifacts {
    /// 文件 patch 记录。
    #[serde(default)]
    pub patches: Vec<PatchRecord>,
    /// 验证命令结果。
    #[serde(default)]
    pub verification_results: Vec<VerificationRecord>,
    /// 执行的命令记录。
    #[serde(default)]
    pub commands_run: Vec<CommandRecord>,
}

/// 一次文件修改记录。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PatchRecord {
    pub path: PathBuf,
    pub diff_summary: String,
    pub applied: bool,
}

/// 一次验证命令的结果。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VerificationRecord {
    pub command: String,
    pub success: bool,
    pub exit_code: Option<i32>,
}

/// 一次命令执行记录。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommandRecord {
    pub command: String,
    pub exit_code: Option<i32>,
    pub success: bool,
}

// ── Conversions ───────────────────────────────────────────────────────

impl From<Report> for WorkerOutcome {
    fn from(report: Report) -> Self {
        let status = match report.outcome {
            RunOutcome::CompletedVerified => WorkerStatus::Completed,
            RunOutcome::CompletedUnverified => WorkerStatus::Submitted,
            RunOutcome::Blocked => WorkerStatus::Blocked,
            RunOutcome::Failed => WorkerStatus::Failed,
            RunOutcome::Interrupted => WorkerStatus::Interrupted,
            RunOutcome::BudgetExhausted => WorkerStatus::Blocked,
        };
        WorkerOutcome {
            worker: report.worker,
            status,
            summary: report.summary,
            findings: report.findings,
            artifacts: WorkerArtifacts::default(),
            attention: report.attention,
            confidence: report.confidence,
            outcome: report.outcome,
            escalation_reason: None,
            trace_events: report.trace_events,
        }
    }
}
