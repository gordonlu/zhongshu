/// Worker output 的通知层级。
///
/// 排序：Ignore < Digest < Notify < Immediate
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum AttentionLevel {
    /// 不通知，仅归档
    Ignore,
    /// 归入日/周报
    Digest,
    /// 桌面通知
    Notify,
    /// 立即打断用户
    Immediate,
}
