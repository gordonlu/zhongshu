use crate::agent::attention::AttentionLevel;

/// Worker 执行的唯一产出。
///
/// 所有 Worker 输出统一格式。Worker 不允许直接回复用户——
/// Report 必须经过 AttentionManager。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Report {
    /// 原始 Task 的 ID
    pub task_id: String,
    /// Worker 名称（对应 profile.name）
    pub worker: String,
    /// 摘要（≤200 字，给 Primary Agent 快速浏览）
    pub summary: String,
    /// 详细发现（Worker 的完整输出）
    pub findings: String,
    /// 置信度（0.0–1.0）
    pub confidence: f64,
    /// 通知层级
    pub attention: AttentionLevel,
}
