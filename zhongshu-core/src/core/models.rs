use serde::{Deserialize, Serialize};
use serde_json::Value;

pub fn id(prefix: &str) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{}", prefix, ts)
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── Observation ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ObservationType {
    UserMessage,
    FileChanged,
    ToolResult,
    ApplicationEvent,
    AgentAction,
}

impl ObservationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ObservationType::UserMessage => "user_message",
            ObservationType::FileChanged => "file_changed",
            ObservationType::ToolResult => "tool_result",
            ObservationType::ApplicationEvent => "application_event",
            ObservationType::AgentAction => "agent_action",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "user_message" => Some(ObservationType::UserMessage),
            "file_changed" => Some(ObservationType::FileChanged),
            "tool_result" => Some(ObservationType::ToolResult),
            "application_event" => Some(ObservationType::ApplicationEvent),
            "agent_action" => Some(ObservationType::AgentAction),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub type_: ObservationType,
    pub content: String,
    pub source: Option<String>,
    pub metadata: Option<Value>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
}

// ── Suggestion ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SuggestionStatus {
    Pending,
    Accepted,
    Rejected,
    Expired,
}

impl SuggestionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SuggestionStatus::Pending => "pending",
            SuggestionStatus::Accepted => "accepted",
            SuggestionStatus::Rejected => "rejected",
            SuggestionStatus::Expired => "expired",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(SuggestionStatus::Pending),
            "accepted" => Some(SuggestionStatus::Accepted),
            "rejected" => Some(SuggestionStatus::Rejected),
            "expired" => Some(SuggestionStatus::Expired),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub id: String,
    pub type_: Option<String>,
    pub content: String,
    pub confidence: f64,
    pub status: SuggestionStatus,
    pub source_observation: Option<String>,
    pub created_at: i64,
}

// ── Goal ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GoalType {
    OneShot,
    Recurring,
    Ongoing,
}

impl GoalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalType::OneShot => "one_shot",
            GoalType::Recurring => "recurring",
            GoalType::Ongoing => "ongoing",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "one_shot" => Some(GoalType::OneShot),
            "recurring" => Some(GoalType::Recurring),
            "ongoing" => Some(GoalType::Ongoing),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GoalStatus {
    Active,
    Paused,
    Completed,
    Archived,
}

impl GoalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalStatus::Active => "active",
            GoalStatus::Paused => "paused",
            GoalStatus::Completed => "completed",
            GoalStatus::Archived => "archived",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "active" => Some(GoalStatus::Active),
            "paused" => Some(GoalStatus::Paused),
            "completed" => Some(GoalStatus::Completed),
            "archived" => Some(GoalStatus::Archived),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub goal_type: GoalType,
    pub status: GoalStatus,
    pub trigger_config: Option<String>,
    pub metadata: Option<Value>,
    pub created_at: i64,
    pub updated_at: i64,
}

// ── Task ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Planning,
    Running,
    WaitingApproval,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::Planning => "planning",
            TaskStatus::Running => "running",
            TaskStatus::WaitingApproval => "waiting_approval",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(TaskStatus::Pending),
            "planning" => Some(TaskStatus::Planning),
            "running" => Some(TaskStatus::Running),
            "waiting_approval" => Some(TaskStatus::WaitingApproval),
            "completed" => Some(TaskStatus::Completed),
            "failed" => Some(TaskStatus::Failed),
            "cancelled" => Some(TaskStatus::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub goal_id: Option<String>,
    pub title: String,
    pub status: TaskStatus,
    pub input: Option<String>,
    pub output: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

// ── Task Step ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
    ToolBlocked,
    VerificationFailed,
}

impl StepStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            StepStatus::Pending => "pending",
            StepStatus::Running => "running",
            StepStatus::Completed => "completed",
            StepStatus::Failed => "failed",
            StepStatus::Skipped => "skipped",
            StepStatus::ToolBlocked => "tool_blocked",
            StepStatus::VerificationFailed => "verification_failed",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(StepStatus::Pending),
            "running" => Some(StepStatus::Running),
            "completed" => Some(StepStatus::Completed),
            "failed" => Some(StepStatus::Failed),
            "skipped" => Some(StepStatus::Skipped),
            "tool_blocked" => Some(StepStatus::ToolBlocked),
            "verification_failed" => Some(StepStatus::VerificationFailed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStep {
    pub id: String,
    pub task_id: String,
    pub step_order: i32,
    pub action: String,
    pub status: StepStatus,
    pub input: Option<String>,
    pub output: Option<String>,
    pub error: Option<String>,
    pub tool_summary: Option<String>,
    pub verification: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

// ── Task Run ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRun {
    pub id: String,
    pub task_id: String,
    pub context: Option<String>,
    pub tool_calls: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

// ── Artifact ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArtifactType {
    File,
    Document,
    Report,
    MessageDraft,
    CalendarEvent,
    CodePatch,
    Knowledge,
}

impl ArtifactType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ArtifactType::File => "file",
            ArtifactType::Document => "document",
            ArtifactType::Report => "report",
            ArtifactType::MessageDraft => "message_draft",
            ArtifactType::CalendarEvent => "calendar_event",
            ArtifactType::CodePatch => "code_patch",
            ArtifactType::Knowledge => "knowledge",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "file" => Some(ArtifactType::File),
            "document" => Some(ArtifactType::Document),
            "report" => Some(ArtifactType::Report),
            "message_draft" => Some(ArtifactType::MessageDraft),
            "calendar_event" => Some(ArtifactType::CalendarEvent),
            "code_patch" => Some(ArtifactType::CodePatch),
            "knowledge" => Some(ArtifactType::Knowledge),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub artifact_type: ArtifactType,
    pub title: Option<String>,
    pub uri: Option<String>,
    pub summary: Option<String>,
    pub metadata: Option<Value>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskArtifact {
    pub task_id: String,
    pub artifact_id: String,
    pub relation: String,
}

// ── Memory Candidate ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryCandidate {
    pub id: String,
    pub content: String,
    pub memory_type: Option<String>,
    pub confidence: f64,
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    pub status: String,
    pub created_at: i64,
}

// ── Memory ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryType {
    Preference,
    Profile,
    Project,
    Decision,
    Procedure,
}

impl MemoryType {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryType::Preference => "preference",
            MemoryType::Profile => "profile",
            MemoryType::Project => "project",
            MemoryType::Decision => "decision",
            MemoryType::Procedure => "procedure",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "preference" => Some(MemoryType::Preference),
            "profile" => Some(MemoryType::Profile),
            "project" => Some(MemoryType::Project),
            "decision" => Some(MemoryType::Decision),
            "procedure" => Some(MemoryType::Procedure),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub memory_type: MemoryType,
    pub content: String,
    pub embedding: Option<Vec<u8>>,
    pub created_at: i64,
    pub updated_at: i64,
}

// ── Event ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventLog {
    pub id: String,
    pub event_type: String,
    pub payload: Option<String>,
    pub created_at: i64,
}
