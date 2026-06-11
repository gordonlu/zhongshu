use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;
use tracing::info;

use crate::event::{Event, EventBus, SourceEvent};

/// 事件源（Source）定义。
///
/// Source 是事件产生者。所有主动行为必须来源于事件。
/// 每个 Source 独立运行，定期 `poll()`，产出 `Event`。
///
/// 当前阶段：
/// - Trait 定义
/// - TimerSource（基础心跳源）
/// - SourceManager（轮询+发布管理）
///
/// 后续（#5 RuleEngine 完成后）：
/// - SystemSource（文件系统/电池/进程）
/// - WebSource（天气/新闻/RSS）
/// - CalendarSource（会议/日程）
#[async_trait]
pub trait Source: Send + Sync {
    /// 唯一标识。
    fn name(&self) -> &str;

    /// 轮询下一个事件。没有新事件时返回 `None`。
    async fn poll(&mut self) -> Option<Event>;
}

/// 定时心跳 Source，按固定间隔产生 `SourceEvent::Tick`。
pub struct TimerSource {
    name: String,
    interval: Duration,
    last: Option<Instant>,
}

impl TimerSource {
    pub fn new(name: impl Into<String>, interval: Duration) -> Self {
        TimerSource {
            name: name.into(),
            interval,
            last: None,
        }
    }
}

#[async_trait]
impl Source for TimerSource {
    fn name(&self) -> &str {
        &self.name
    }

    async fn poll(&mut self) -> Option<Event> {
        let should_fire = match self.last {
            Some(last) => last.elapsed() >= self.interval,
            None => true,
        };
        if should_fire {
            self.last = Some(Instant::now());
            Some(Event::Source(SourceEvent::Tick {
                name: self.name.clone(),
            }))
        } else {
            None
        }
    }
}

/// 磁盘使用率 Source —— 监控磁盘使用，超过阈值时触发事件。
pub struct DiskUsageSource {
    name: String,
    path: PathBuf,
    threshold: f64,
    last_fired: Option<Instant>,
    cooldown: Duration,
}

impl DiskUsageSource {
    /// `threshold`: 0.0–1.0，使用率超过此值触发告警。
    pub fn new(name: impl Into<String>, path: impl Into<PathBuf>, threshold: f64, cooldown: Duration) -> Self {
        DiskUsageSource {
            name: name.into(),
            path: path.into(),
            threshold,
            last_fired: None,
            cooldown,
        }
    }

    fn usage_pct(&self) -> Option<f64> {
        #[cfg(target_os = "linux")]
        {
            let path_str = self.path.to_str()?;
            let output = std::process::Command::new("df")
                .arg("--output=pcent")
                .arg(path_str)
                .output().ok()?;
            let stdout = String::from_utf8(output.stdout).ok()?;
            let line = stdout.lines().nth(1)?;
            let pct: f64 = line.trim().trim_end_matches('%').parse().ok()?;
            Some(pct / 100.0)
        }
        #[cfg(target_os = "windows")]
        {
            use winapi::um::fileapi::GetDiskFreeSpaceExW;
            use winapi::shared::ntdef::ULARGE_INTEGER;
            use std::ffi::OsStr;
            use std::os::windows::ffi::OsStrExt;
            let path: Vec<u16> = OsStr::new(self.path.as_os_str())
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            // SAFETY: ULARGE_INTEGER is a union; initialising to zero is safe.
            let mut free_bytes: ULARGE_INTEGER = unsafe { std::mem::zeroed() };
            let mut total_bytes: ULARGE_INTEGER = unsafe { std::mem::zeroed() };
            let mut _total_free: ULARGE_INTEGER = unsafe { std::mem::zeroed() };
            if unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut free_bytes, &mut total_bytes, &mut _total_free) } == 0 {
                return None;
            }
            // SAFETY: reading the anonymous QuadPart field of an initialised union.
            let total = unsafe { *total_bytes.QuadPart() } as u64;
            if total == 0 {
                return None;
            }
            let free = unsafe { *free_bytes.QuadPart() } as u64;
            let used = total - free;
            Some(used as f64 / total as f64)
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        None
    }
}

#[async_trait]
impl Source for DiskUsageSource {
    fn name(&self) -> &str {
        &self.name
    }

    async fn poll(&mut self) -> Option<Event> {
        let in_cooldown = self.last_fired.is_some_and(|t| t.elapsed() < self.cooldown);
        if in_cooldown {
            return None;
        }
        let usage = self.usage_pct();
        let usage = match usage {
            Some(u) => u,
            None => {
                static WARNED: std::sync::Once = std::sync::Once::new();
                WARNED.call_once(|| {
                    #[cfg(target_os = "linux")]
                    tracing::warn!(path = %self.path.display(), "DiskUsageSource: df failed");
                    #[cfg(target_os = "windows")]
                    tracing::warn!(path = %self.path.display(), "DiskUsageSource: GetDiskFreeSpaceExW failed");
                    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
                    tracing::warn!("DiskUsageSource not supported on this platform");
                });
                return None;
            }
        };
        if usage >= self.threshold {
            self.last_fired = Some(Instant::now());
            Some(Event::Source(SourceEvent::DiskUsage {
                path: self.path.to_string_lossy().to_string(),
                usage_pct: usage,
            }))
        } else {
            None
        }
    }
}

/// 电池电量 Source —— 监控电池电量，低于阈值时触发事件。
pub struct BatterySource {
    name: String,
    low_threshold: u8,
    last_fired: Option<Instant>,
    cooldown: Duration,
}

impl BatterySource {
    pub fn new(name: impl Into<String>, low_threshold: u8, cooldown: Duration) -> Self {
        BatterySource {
            name: name.into(),
            low_threshold,
            last_fired: None,
            cooldown,
        }
    }

    fn battery_level() -> Option<u8> {
        #[cfg(target_os = "linux")]
        {
            let cap = std::fs::read_to_string("/sys/class/power_supply/BAT0/capacity").ok()?;
            return cap.trim().parse().ok();
        }
        #[cfg(target_os = "windows")]
        {
            use winapi::um::winbase::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
            let mut status: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
            if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
                return None;
            }
            if status.BatteryFlag == 255 {
                return None;
            }
            return Some(status.BatteryLifePercent);
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        None
    }

    fn is_charging() -> bool {
        #[cfg(target_os = "linux")]
        {
            return std::fs::read_to_string("/sys/class/power_supply/BAT0/status")
                .ok().is_some_and(|s| s.trim() == "Charging");
        }
        #[cfg(target_os = "windows")]
        {
            use winapi::um::winbase::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
            let mut status: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
            if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
                return false;
            }
            return status.ACLineStatus == 1;
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        false
    }
}

#[async_trait]
impl Source for BatterySource {
    fn name(&self) -> &str {
        &self.name
    }

    async fn poll(&mut self) -> Option<Event> {
        if Self::is_charging() {
            self.last_fired = None;
            return None;
        }
        let in_cooldown = self.last_fired.is_some_and(|t| t.elapsed() < self.cooldown);
        if in_cooldown {
            return None;
        }
        let level = Self::battery_level()?;
        if level <= self.low_threshold {
            self.last_fired = Some(Instant::now());
            Some(Event::Source(SourceEvent::BatteryLow { level }))
        } else {
            None
        }
    }
}

/// Source 管理器 —— 定期轮询所有已注册 Source 并发布事件到 EventBus。

pub struct SourceManager {
    sources: Vec<Box<dyn Source>>,
    eb: EventBus,
    interval: Duration,
}

impl SourceManager {
    pub fn new(eb: EventBus) -> Self {
        SourceManager {
            sources: Vec::new(),
            eb,
            interval: Duration::from_secs(1),
        }
    }

    /// 设置轮询间隔（默认 1 秒）。
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    pub fn register(&mut self, source: impl Source + 'static) {
        info!(name = %source.name(), "registered source");
        self.sources.push(Box::new(source));
    }

    /// 启动后台轮询循环。
    ///
    /// 每次 tick 遍历所有 Source 并调用 `poll()`，
    /// 有事件则发布到 EventBus。
    pub fn spawn(mut self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(self.interval);
            loop {
                tick.tick().await;
                for source in &mut self.sources {
                    match tokio::time::timeout(Duration::from_secs(5), source.poll()).await {
                        Ok(Some(event)) => {
                            tracing::debug!(source = %source.name(), "source event");
                            self.eb.publish(event);
                        }
                        Ok(None) => {}
                        Err(_) => {
                            tracing::warn!(source = %source.name(), "source poll timed out");
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::SourceEvent;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct TestSource {
        name: String,
        fired: Arc<AtomicBool>,
    }

    #[async_trait]
    impl Source for TestSource {
        fn name(&self) -> &str {
            &self.name
        }

        async fn poll(&mut self) -> Option<Event> {
            if self.fired.swap(false, Ordering::SeqCst) {
                Some(Event::Source(SourceEvent::Tick {
                    name: self.name.clone(),
                }))
            } else {
                None
            }
        }
    }

    #[tokio::test]
    async fn timer_source_fires_at_interval() {
        let mut src = TimerSource::new("test", Duration::from_millis(10));
        tokio::time::sleep(Duration::from_millis(50)).await;
        let event = src.poll().await;
        assert!(event.is_some(), "should fire after interval");
        if let Some(Event::Source(SourceEvent::Tick { name })) = event {
            assert_eq!(name, "test");
        } else {
            panic!("unexpected event");
        }
    }

    #[tokio::test]
    async fn timer_source_does_not_fire_twice() {
        let mut src = TimerSource::new("t", Duration::from_secs(60));
        assert!(src.poll().await.is_some(), "should fire on first poll (Option<Instant> init to None)");
        assert!(src.poll().await.is_none(), "should not fire twice in a row");
    }

    #[tokio::test]
    async fn source_manager_polls_and_publishes() {
        let eb = EventBus::new(16);
        let mut rx = eb.subscribe();
        let fired = Arc::new(AtomicBool::new(true));

        let mut mgr = SourceManager::new(eb);
        mgr.register(TestSource {
            name: "test".into(),
            fired: fired.clone(),
        });

        // Manually poll (not spawn) for deterministic test.
        for source in &mut mgr.sources {
            if let Some(event) = source.poll().await {
                mgr.eb.publish(event);
            }
        }

        let received = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(received.is_ok(), "should receive published event");
    }
}
