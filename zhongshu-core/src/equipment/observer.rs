// ── 军器监 Observer ──────────────────────────────────────────────────
//
// 隐私边界（约束设计，不可违反）：
//
//   ✅ 中书可见：用户与中书交互的行为
//      - 工具调用（名称、成功/失败）
//      - 用户发给中书的文字消息
//      - Agent 状态变化
//
//   ❌ 中书不可见（除非用户显式授权）：
//      - 浏览器历史
//      - 系统进程
//      - 键盘记录（仅聊天输入框内的文字被记录为用户消息）
//      - 屏幕内容（除非 screenshot tool 被调用）
//      - 其他应用行为
//
//   Observer 只通过 EventBus 订阅中书内部事件（tool calls、agent state），
//   不主动扫描系统。用户消息由 orb 层调用 record_user_message() 传入，
//   不窃听系统输入。
// ─────────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::equipment::Manifest;
use crate::event::{AgentState, Event, ToolEvent};

/// Maximum observations kept in the ring buffer.
const MAX_OBSERVATIONS: usize = 5000;

/// How many seconds in each window for pattern analysis.
const SECS_PER_HOUR: u64 = 3600;

#[derive(Debug, Clone)]
pub struct Observation {
    pub timestamp: u64,
    pub kind: ObservationKind,
}

#[derive(Debug, Clone)]
pub enum ObservationKind {
    /// A tool was invoked.
    ToolStarted { name: String },
    /// A tool finished (success = true/false).
    ToolCompleted { name: String, success: bool },
    /// Agent state changed.
    AgentStateChange { from: AgentState, to: AgentState },
    /// Something the user typed (truncated to 200 chars).
    UserMessage { content: String },
}

/// Ring buffer that stores the last N observations.
#[derive(Debug)]
pub struct ObservationStore {
    buffer: Vec<Observation>,
    cursor: usize,
}

impl ObservationStore {
    pub fn new() -> Self {
        ObservationStore {
            buffer: Vec::with_capacity(MAX_OBSERVATIONS),
            cursor: 0,
        }
    }

    pub fn push(&mut self, obs: Observation) {
        if self.buffer.len() < MAX_OBSERVATIONS {
            self.buffer.push(obs);
        } else {
            // Replace the oldest (assumes cursor wraps around)
            let idx = self.cursor % MAX_OBSERVATIONS;
            self.buffer[idx] = obs;
        }
        self.cursor += 1;
    }

    pub fn all(&self) -> &[Observation] {
        &self.buffer
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Observations since a given unix timestamp.
    pub fn since(&self, since_ts: u64) -> Vec<&Observation> {
        self.buffer
            .iter()
            .filter(|o| o.timestamp >= since_ts)
            .collect()
    }

    /// Observations from the last N hours.
    pub fn last_hours(&self, hours: u64) -> Vec<&Observation> {
        let cutoff = now() - hours * SECS_PER_HOUR;
        self.since(cutoff)
    }

    /// Observations from today (since midnight).
    pub fn today(&self) -> Vec<&Observation> {
        let midnight = (now() / 86400) * 86400;
        self.since(midnight)
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    /// Count tool calls, keyed by tool name.
    fn tool_call_counts(window: &[&Observation]) -> HashMap<String, u32> {
        let mut counts: HashMap<String, u32> = HashMap::new();
        for o in window {
            if let ObservationKind::ToolStarted { name } = &o.kind {
                *counts.entry(name.clone()).or_insert(0) += 1;
            }
        }
        counts
    }

    /// Count tool failures, keyed by tool name.
    fn tool_fail_counts(window: &[&Observation]) -> HashMap<String, (u32, u32)> {
        let mut fails: HashMap<String, u32> = HashMap::new();
        let mut totals: HashMap<String, u32> = HashMap::new();
        for o in window {
            if let ObservationKind::ToolCompleted { name, success } = &o.kind {
                *totals.entry(name.clone()).or_insert(0) += 1;
                if !success {
                    *fails.entry(name.clone()).or_insert(0) += 1;
                }
            }
        }
        let mut result: HashMap<String, (u32, u32)> = HashMap::new();
        let all_names: Vec<String> = fails.keys().chain(totals.keys()).cloned().collect();
        for name in all_names {
            let f = fails.get(&name).copied().unwrap_or(0);
            let t = totals.get(&name).copied().unwrap_or(0);
            result.insert(name, (f, t));
        }
        result
    }

    /// Count user messages by simple category.
    fn message_categories(&self, window: &[&Observation]) -> Vec<(&str, u32)> {
        let keywords: [(&str, &[&str]); 6] = [
            (
                "查询/搜索",
                &["查", "搜索", "找", "搜", "search", "find", "look up"],
            ),
            (
                "文件操作",
                &[
                    "文件", "创建", "编辑", "写", "读", "复制", "移动", "删除", "file", "write",
                    "read",
                ],
            ),
            (
                "开发/代码",
                &[
                    "代码",
                    "编译",
                    "测试",
                    "git",
                    "cargo",
                    "rust",
                    "python",
                    "代码审查",
                    "debug",
                ],
            ),
            (
                "系统管理",
                &[
                    "安装", "配置", "设置", "进程", "磁盘", "内存", "cpu", "系统",
                ],
            ),
            (
                "股价/比赛",
                &[
                    "股价", "股票", "行情", "比赛", "比分", "score", "stock", "price",
                ],
            ),
            (
                "日常",
                &["天气", "新闻", "提醒", "定时", "每天早上", "每天晚上"],
            ),
        ];
        let mut counts: HashMap<&str, u32> = HashMap::new();
        for o in window {
            if let ObservationKind::UserMessage { content } = &o.kind {
                let lower = content.to_lowercase();
                for (cat, kws) in &keywords {
                    if kws.iter().any(|kw| lower.contains(kw)) {
                        *counts.entry(cat).or_insert(0) += 1;
                        break;
                    }
                }
            }
        }
        let mut result: Vec<_> = counts.into_iter().collect();
        result.sort_by(|a, b| b.1.cmp(&a.1));
        result
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Strip common sensitive patterns from text before storing in observations.
/// API keys, tokens, emails, IPs, home paths, and key=value secrets.
fn scrub_message(text: &str) -> String {
    let mut s = text.to_string();

    // API keys: sk-... (OpenAI), ghp_... (GitHub PAT), github_pat_..., etc.
    s = replace_pattern(
        &s,
        &["sk-", "sk-or-", "ghp_", "gho_", "github_pat_", "ghr_"],
        10,
    );

    // Token/secret patterns in text: "key=xxx", "token=xxx", "secret=xxx"
    let kv_triggers = [
        "key=",
        "token=",
        "secret=",
        "password=",
        "passwd=",
        "apikey=",
        "api_key=",
        "api-key=",
    ];
    for trig in kv_triggers {
        s = replace_after(&s, trig, 6);
    }

    // Email addresses: replace user@domain.tld
    s = replace_email(&s);

    // IP addresses (simplified: 4 groups of 1-3 digits separated by dots)
    s = replace_ip(&s);

    // Home directory paths
    let home_indicators = ["/home/", "C:\\Users\\", "/Users/"];
    for ind in home_indicators {
        s = replace_after(&s, ind, 10);
    }

    s
}

/// Replace a known prefix + trailing alphanumeric/`-`/`_` chars with `[REDACTED]`.
fn replace_pattern(s: &str, prefixes: &[&str], min_trailing: usize) -> String {
    let mut result = s.to_string();
    for &prefix in prefixes {
        let mut start = 0;
        while let Some(pos) = result[start..].find(prefix) {
            let abs_pos = start + pos;
            let after = &result[abs_pos + prefix.len()..];
            let end = after
                .find(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
                .unwrap_or(after.len());
            if end >= min_trailing {
                let replacement = format!("{}[REDACTED]", prefix);
                result.replace_range(abs_pos..abs_pos + prefix.len() + end, &replacement);
                start = abs_pos + replacement.len();
            } else {
                start = abs_pos + 1;
            }
        }
    }
    result
}

/// Replace content after a trigger word until next space/comma/quote.
fn replace_after(s: &str, trigger: &str, min_val_len: usize) -> String {
    let mut result = s.to_string();
    let mut start = 0;
    while let Some(pos) = result[start..].find(trigger) {
        let abs_pos = start + pos;
        let after = &result[abs_pos + trigger.len()..];
        let end = after
            .find(|c: char| c.is_whitespace() || c == ',' || c == '"' || c == '\'')
            .unwrap_or(after.len());
        if end >= min_val_len {
            let replacement = format!("{}[REDACTED]", trigger);
            result.replace_range(abs_pos..abs_pos + trigger.len() + end, &replacement);
            start = abs_pos + replacement.len();
        } else {
            start = abs_pos + 1;
        }
    }
    result
}

/// Replace email addresses with [REDACTED].
fn replace_email(s: &str) -> String {
    let mut result = s.to_string();
    // Find @ preceded by word chars and followed by domain
    let mut start = 0;
    while let Some(pos) = result[start..].find('@') {
        let abs_pos = start + pos;
        // Look backwards for start of local part
        let local_start = result[..abs_pos]
            .rfind(|c: char| !c.is_alphanumeric() && c != '.' && c != '_' && c != '-')
            .map(|i| i + 1)
            .unwrap_or(0);
        // Look forward for end of domain
        let after = &result[abs_pos + 1..];
        let domain_end = after
            .find(|c: char| !c.is_alphanumeric() && c != '.' && c != '-')
            .unwrap_or(after.len());
        if domain_end >= 3 && abs_pos - local_start >= 1 {
            let email_end = abs_pos + 1 + domain_end;
            result.replace_range(local_start..email_end, "[EMAIL REDACTED]");
            start = local_start + "[EMAIL REDACTED]".len();
        } else {
            start = abs_pos + 1;
        }
    }
    result
}

/// Replace IPv4 addresses with [REDACTED].
fn replace_ip(s: &str) -> String {
    use std::net::Ipv4Addr;
    let mut result = s.to_string();
    let mut start = 0;
    while let Some(pos) = result[start..].find(|c: char| c.is_ascii_digit()) {
        let abs_pos = start + pos;
        // Find the end of this number sequence
        let rest = &result[abs_pos..];
        let num_end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        let candidate = &rest[..num_end];
        // Check if it looks like an IP (at least 7 chars, contains 3 dots)
        if candidate.len() >= 7 && candidate.matches('.').count() == 3 {
            if candidate.parse::<Ipv4Addr>().is_ok() {
                result.replace_range(abs_pos..abs_pos + num_end, "[IP REDACTED]");
                start = abs_pos + "[IP REDACTED]".len();
                continue;
            }
        }
        start = abs_pos + 1;
    }
    result
}

/// Subscribes to EventBus and records tool/agent/user observations.
/// Shared via Arc<Mutex<>> between the background task and callers.
pub struct EquipmentObserver {
    store: ObservationStore,
}

impl EquipmentObserver {
    pub fn new() -> Self {
        EquipmentObserver {
            store: ObservationStore::new(),
        }
    }

    pub fn store(&self) -> &ObservationStore {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut ObservationStore {
        &mut self.store
    }

    /// Record a user message observation (called from orb layer).
    pub fn record_user_message(&mut self, content: &str) {
        let scrubbed = scrub_message(content);
        let truncated = if scrubbed.len() > 200 {
            format!("{}...", &scrubbed[..197])
        } else {
            scrubbed
        };
        self.store.push(Observation {
            timestamp: now(),
            kind: ObservationKind::UserMessage { content: truncated },
        });
    }

    /// Consume one EventBus event and record if relevant.
    pub fn observe(&mut self, event: &Event) {
        let ts = now();
        match event {
            Event::Tool(ToolEvent::Started { name }) => {
                self.store.push(Observation {
                    timestamp: ts,
                    kind: ObservationKind::ToolStarted { name: name.clone() },
                });
            }
            Event::Tool(ToolEvent::Completed { name, success }) => {
                self.store.push(Observation {
                    timestamp: ts,
                    kind: ObservationKind::ToolCompleted {
                        name: name.clone(),
                        success: *success,
                    },
                });
            }
            Event::Agent(crate::event::AgentEvent::StateChanged { from, to }) => {
                self.store.push(Observation {
                    timestamp: ts,
                    kind: ObservationKind::AgentStateChange {
                        from: *from,
                        to: *to,
                    },
                });
            }
            _ => {}
        }
    }

    /// Spawn a background task that subscribes to EventBus and observes.
    /// Returns an `Arc<Mutex<EquipmentObserver>>` for the orb layer to call
    /// `record_user_message()` on the main thread.
    pub fn spawn(
        self,
        eb: &crate::event::EventBus,
    ) -> (
        tokio::task::JoinHandle<()>,
        std::sync::Arc<std::sync::Mutex<EquipmentObserver>>,
    ) {
        let observer = std::sync::Arc::new(std::sync::Mutex::new(self));
        let clone = observer.clone();
        let mut rx = eb.subscribe();
        let handle = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if let Ok(mut guard) = clone.lock() {
                            guard.observe(&event);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("observer lagged: {n}");
                    }
                    Err(_) => break,
                }
            }
        });
        (handle, observer)
    }

    /// Tool usage stats for a given time window.
    fn tool_stats(&self, window: &[&Observation]) -> String {
        let counts = ObservationStore::tool_call_counts(window);
        let fails = ObservationStore::tool_fail_counts(window);

        if counts.is_empty() {
            return String::new();
        }

        let mut out = String::from("### 工具调用\n\n");
        let mut sorted: Vec<_> = counts.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        for (name, count) in sorted.iter().take(10) {
            let fail_info = fails
                .get(name.as_str())
                .map(|(f, t)| {
                    if *t > 0 {
                        format!(
                            " (失败 {}/{} = {:.0}%)",
                            f,
                            t,
                            *f as f64 / *t as f64 * 100.0
                        )
                    } else {
                        String::new()
                    }
                })
                .unwrap_or_default();
            out.push_str(&format!("- {}: {} 次{}\n", name, count, fail_info));
        }
        out
    }

    /// User message category summary for a given time window.
    fn user_patterns(&self, window: &[&Observation]) -> String {
        let msgs: Vec<_> = window
            .iter()
            .filter(|o| matches!(o.kind, ObservationKind::UserMessage { .. }))
            .collect();
        if msgs.is_empty() {
            return String::new();
        }
        let mut out = String::from("### 用户提问类型\n\n");
        let cats = self.store.message_categories(window);
        for (cat, count) in &cats {
            out.push_str(&format!("- {}: {} 次\n", cat, count));
        }
        out.push_str(&format!("\n共 {} 条用户消息", msgs.len()));
        out
    }

    /// Recent user messages (raw, truncated).
    fn recent_user_messages(&self, window: &[&Observation], max: usize) -> String {
        let msgs: Vec<_> = window
            .iter()
            .filter_map(|o| {
                if let ObservationKind::UserMessage { content } = &o.kind {
                    Some(content.as_str())
                } else {
                    None
                }
            })
            .rev()
            .take(max)
            .collect();
        if msgs.is_empty() {
            return String::new();
        }
        let mut out = String::from("### 用户最近提问\n\n");
        for (i, msg) in msgs.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i + 1, msg));
        }
        out
    }

    /// Generate a period usage report (e.g. daily/weekly digest).
    pub fn period_report(&self, label: &str, hours: u64) -> String {
        let window = self.store.last_hours(hours);
        let tool_stats = self.tool_stats(&window);
        let user_pats = self.user_patterns(&window);
        let recent_msgs = self.recent_user_messages(&window, 10);

        let mut report = format!("## 军器监观察报告（{label}）\n\n");
        report.push_str(&format!("观察窗口: 最近 {} 小时\n\n", hours));

        if !tool_stats.is_empty() {
            report.push_str(&tool_stats);
            report.push('\n');
        }
        if !user_pats.is_empty() {
            report.push_str(&user_pats);
            report.push('\n');
        }
        if !recent_msgs.is_empty() {
            report.push_str(&recent_msgs);
            report.push('\n');
        }

        if tool_stats.is_empty() && user_pats.is_empty() {
            report.push_str("（暂无观察数据）");
        }

        report
    }

    /// Generate a prompt for the LLM to propose new equipment based on
    /// observed user patterns. Returns None if insufficient data.
    pub fn equipment_proposal_prompt(&self) -> Option<String> {
        if !self.has_sufficient_data() {
            return None;
        }
        Some(self.build_proposal_prompt())
    }

    /// Minimum data required before proposing equipment.
    fn has_sufficient_data(&self) -> bool {
        let all = self.store.all();
        if all.len() < 30 {
            return false;
        }
        let tool_starts = all
            .iter()
            .filter(|o| matches!(o.kind, ObservationKind::ToolStarted { .. }))
            .count();
        if tool_starts < 10 {
            return false;
        }
        let user_msgs = all
            .iter()
            .filter(|o| matches!(o.kind, ObservationKind::UserMessage { .. }))
            .count();
        if user_msgs < 5 {
            return false;
        }
        // Check that observations span at least 2 different days
        let mut days: Vec<u64> = all.iter().map(|o| o.timestamp / 86400).collect();
        days.sort();
        days.dedup();
        days.len() >= 2
    }

    fn build_proposal_prompt(&self) -> String {
        let daily = self.period_report("过去 24 小时", 24);
        let weekly = self.period_report("过去 7 天", 168);

        format!(
            r#"{daily}

{weekly}

---

## 任务

根据以上观察数据，判断是否有值得为用户**升级或新建**装备（equipment）的模式。

装备是 `~/.config/zhongshu/equipment/<name>/` 目录下的一个包。当前自动进化链路只支持生成 **skill 装备**：

```
manifest.json    # 必选：装备元数据
prompt.md        # 由中书根据 manifest 自动生成并注入 system prompt
```

装备类型固定为：
- `skill` — 可复用的技能（触发条件、处理原则、推荐工具）

### 判断规则

1. 重复 ≥ 3 次相同模式 → 应该生成装备
2. "股价"或"比赛"类查询如果每日出现 → 有定时装备需求
3. 相似工具调用序列出现多次 → 可抽象为 workflow

### 关键约束：禁止制造重复装备 ⚠️

这是最重要的规则：**不要制造功能重复的装备**。

- 用户不需要 5 个不同的"查股价"装备
- 如果已经有做类似事情的装备，**升级它**，而不是新建
- 输出提议前先思考：这个功能是不是已经有装备覆盖了？
- 宁可不提议，也不要提议重复的装备
- 不确定时保守处理

### 版本规则

- 升级已有装备：版本号递增（1.0.0 → 1.1.0）
- 新建装备：从 1.0.0 开始

### 输出格式

没有合适的装备建议 → 只输出 `无需装备`

有合适的装备（新建或升级）→ 输出 JSON 格式的 manifest.json：

```json
{{
  "name": "<装备名>",
  "version": "1.0.0",
  "type": "skill",
  "description": "<说明>",
  "tools": ["<需要的工具名>"],
  "permissions": {{
    "shell": {{
      "allowed": false,
      "allowed_commands": ["<允许的命令>"]
    }}
  }}
}}
```

然后附上简短的说明：升级/新建的理由，以及预期效果。"#
        )
    }

    /// Standard usage report (for backward compat).
    pub fn usage_report(&self) -> String {
        self.period_report("全部时间", 24 * 365 * 10)
    }
}

/// Parse LLM response to an equipment proposal into a Manifest.
/// Returns None if the LLM declined ("无需装备") or parsing failed.
pub fn parse_proposal_response(text: &str) -> Option<Manifest> {
    let trimmed = text.trim();
    if trimmed.contains("无需装备") {
        return None;
    }
    // Try to extract JSON from markdown code block first.
    let json_str = if let Some(start) = trimmed.find("```json") {
        let after = &trimmed[start + 7..];
        let end = after.find("```").unwrap_or(after.len());
        after[..end].trim()
    } else if let Some(start) = trimmed.find('{') {
        let end = trimmed.rfind('}')?;
        &trimmed[start..=end]
    } else {
        return None;
    };
    serde_json::from_str(json_str).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_ring_buffer_capacity() {
        let mut store = ObservationStore::new();
        for i in 0..(MAX_OBSERVATIONS + 100) {
            store.push(Observation {
                timestamp: i as u64,
                kind: ObservationKind::ToolStarted {
                    name: "test".into(),
                },
            });
        }
        assert_eq!(store.len(), MAX_OBSERVATIONS);
    }

    #[test]
    fn store_recent() {
        let mut store = ObservationStore::new();
        for i in 0..10 {
            store.push(Observation {
                timestamp: i as u64,
                kind: ObservationKind::ToolStarted {
                    name: format!("t{i}"),
                },
            });
        }
        let recent = store.since(7);
        assert_eq!(recent.len(), 3); // timestamps 7,8,9
    }

    #[test]
    fn store_last_hours() {
        let mut store = ObservationStore::new();
        let now_ts = now();
        // 3 hours ago
        store.push(Observation {
            timestamp: now_ts - 10800,
            kind: ObservationKind::ToolStarted { name: "old".into() },
        });
        // now
        store.push(Observation {
            timestamp: now_ts,
            kind: ObservationKind::ToolStarted {
                name: "recent".into(),
            },
        });
        let last2h = store.last_hours(2);
        assert_eq!(last2h.len(), 1);
        assert!(
            matches!(&last2h[0].kind, ObservationKind::ToolStarted { name } if name == "recent")
        );
    }

    #[test]
    fn observer_records_tool_events() {
        let mut obs = EquipmentObserver::new();
        obs.observe(&Event::Tool(ToolEvent::Started {
            name: "shell".into(),
        }));
        obs.observe(&Event::Tool(ToolEvent::Completed {
            name: "shell".into(),
            success: true,
        }));
        obs.observe(&Event::Tool(ToolEvent::Started {
            name: "read_file".into(),
        }));
        obs.observe(&Event::Tool(ToolEvent::Completed {
            name: "read_file".into(),
            success: false,
        }));
        assert_eq!(obs.store.len(), 4);
        let report = obs.usage_report();
        assert!(report.contains("shell"));
        assert!(report.contains("read_file"));
        assert!(report.contains("失败率") || report.contains("失败"));
    }

    #[test]
    fn observer_records_user_messages() {
        let mut obs = EquipmentObserver::new();
        obs.record_user_message("今天股价多少");
        obs.record_user_message("帮我查一下今天的比赛结果");
        obs.record_user_message("写个 rust 程序");

        let report = obs.usage_report();
        assert!(report.contains("用户提问"));
    }

    #[test]
    fn usage_report_empty_when_no_observations() {
        let obs = EquipmentObserver::new();
        let report = obs.usage_report();
        // Still returns a report header, but with "暂无观察数据"
        assert!(report.contains("军器监观察报告") || report.contains("暂无观察数据"));
    }

    #[test]
    fn equipment_proposal_prompt_generates() {
        let mut obs = EquipmentObserver::new();
        // Simulate data spread across 3 days using observe() for tool events
        // (user messages here use current timestamp, but tool events also create
        // observations at the same time — the 30+ total and 10+ tool starts
        // should pass, but the 2-day check may fail since all timestamps are "now".
        //
        // Add enough tool + user observations to clear thresholds.
        for i in 0..6 {
            obs.observe(&Event::Tool(ToolEvent::Started {
                name: "shell".into(),
            }));
            obs.observe(&Event::Tool(ToolEvent::Completed {
                name: "shell".into(),
                success: true,
            }));
            obs.observe(&Event::Tool(ToolEvent::Started {
                name: "web_search".into(),
            }));
            obs.observe(&Event::Tool(ToolEvent::Completed {
                name: "web_search".into(),
                success: true,
            }));
        }
        for i in 0..5 {
            obs.record_user_message(&format!("今天股价多少 session {}", i));
            obs.record_user_message(&format!("帮我查比赛结果 session {}", i));
        }
        // Manually push observations with different day timestamps
        let two_days_ago = now() - 172800;
        let yesterday = now() - 86400;
        for _ in 0..5 {
            obs.store_mut().push(Observation {
                timestamp: two_days_ago,
                kind: ObservationKind::ToolStarted {
                    name: "read_file".into(),
                },
            });
            obs.store_mut().push(Observation {
                timestamp: yesterday,
                kind: ObservationKind::UserMessage {
                    content: "查股价".into(),
                },
            });
        }
        let prompt = obs.equipment_proposal_prompt();
        assert!(
            prompt.is_some(),
            "should have sufficient data, got None. total={}",
            obs.store.len()
        );
        let text = prompt.unwrap();
        assert!(text.contains("manifest.json"));
        assert!(text.contains("股价"));
    }

    #[test]
    fn scrub_api_keys() {
        let s = scrub_message("my key is sk-proj-abc123def456 and also ghp_abc123def456ghi");
        assert!(!s.contains("sk-proj-abc123def456"), "API key leaked: {s}");
        assert!(s.contains("[REDACTED]"), "no redaction marker: {s}");
    }

    #[test]
    fn scrub_email() {
        let s = scrub_message("contact me at user@example.com please");
        assert!(!s.contains("user@example.com"), "email leaked: {s}");
        assert!(s.contains("[EMAIL REDACTED]"));
    }

    #[test]
    fn scrub_ip() {
        let s = scrub_message("server at 192.168.1.100 is down");
        assert!(!s.contains("192.168.1.100"), "IP leaked: {s}");
        assert!(s.contains("[IP REDACTED]"));
    }

    #[test]
    fn scrub_home_path() {
        let s = scrub_message("check /home/alice/.ssh/config");
        assert!(!s.contains("/home/alice"), "home path leaked: {s}");
        assert!(s.contains("[REDACTED]"));
    }

    #[test]
    fn scrub_password_kv() {
        let s = scrub_message("db password=super-secret-123");
        assert!(!s.contains("super-secret-123"), "password leaked: {s}");
        assert!(s.contains("[REDACTED]"));
    }

    #[test]
    fn scrub_normal_text_preserved() {
        let s = scrub_message("今天股价多少？帮我查一下苹果的股票");
        assert_eq!(s, "今天股价多少？帮我查一下苹果的股票");
    }

    #[test]
    fn proposal_returns_none_when_insufficient_data() {
        let obs = EquipmentObserver::new();
        assert!(obs.equipment_proposal_prompt().is_none());
    }

    #[test]
    fn has_sufficient_data_false_by_default() {
        let obs = EquipmentObserver::new();
        assert!(!obs.has_sufficient_data());
    }
}
