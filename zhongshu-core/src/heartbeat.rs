use std::time::Duration;

use tracing::info;

/// Heartbeat —— Runtime 维护服务。
///
/// Heartbeat 不等于 Agent。Heartbeat 是 Runtime 服务。
///
/// 职责：
/// - Worker 超时检查
/// - Source 健康检查
/// - 缓存清理
/// - 重试调度
/// - 指标采集
/// - Rule Reload
///
/// Heartbeat 不调用 LLM。
///
/// 当前阶段：骨架实现，维护钩子已预留但尚未接入具体检查逻辑。
/// 后续逐步接入各检查项。
pub struct Heartbeat {
    interval: Duration,
}

impl Heartbeat {
    pub fn new(interval: Duration) -> Self {
        Heartbeat { interval }
    }

    /// 启动心跳循环。
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(self.interval);
            tick.tick().await;
            loop {
                tick.tick().await;
                Self::perform_maintenance().await;
            }
        })
    }

    async fn perform_maintenance() {
        info!("heartbeat: maintenance cycle");
    }
}

impl Default for Heartbeat {
    fn default() -> Self {
        Heartbeat::new(Duration::from_secs(30))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_creates_with_default_interval() {
        let hb = Heartbeat::default();
        assert_eq!(hb.interval, Duration::from_secs(30));
    }

    #[test]
    fn heartbeat_creates_with_custom_interval() {
        let hb = Heartbeat::new(Duration::from_secs(10));
        assert_eq!(hb.interval, Duration::from_secs(10));
    }
}
