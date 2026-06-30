use serde::{Deserialize, Serialize};

use crate::tool::{Tool, ToolOutput, ToolStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEffect {
    Read,
    Write,
    Network,
    Process,
    Browser,
    System,
    Memory,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceScope {
    WorkspaceOnly,
    CurrentDirectoryOnly,
    External,
    Unrestricted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub effect: ToolEffect,
    pub read_only: bool,
    pub destructive: bool,
    pub workspace_scope: WorkspaceScope,
    pub supports_concurrent_execution: bool,
    pub requires_approval: bool,
}

impl ToolSpec {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            effect: ToolEffect::Unknown,
            read_only: false,
            destructive: false,
            workspace_scope: WorkspaceScope::Unrestricted,
            supports_concurrent_execution: false,
            requires_approval: true,
        }
    }

    pub fn from_tool<T: Tool + ?Sized>(tool: &T) -> Self {
        let name = tool.name();
        let effect = infer_effect(name);
        let read_only = infer_read_only(name, effect);
        let destructive = infer_destructive(name, effect);
        let workspace_scope = infer_workspace_scope(name, effect);
        Self {
            name: name.to_string(),
            effect,
            read_only,
            destructive,
            workspace_scope,
            supports_concurrent_execution: read_only && !destructive,
            requires_approval: destructive || matches!(effect, ToolEffect::System),
        }
    }

    pub fn with_effect(mut self, effect: ToolEffect) -> Self {
        self.effect = effect;
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        if read_only && !self.destructive {
            self.supports_concurrent_execution = true;
            self.requires_approval = false;
        }
        self
    }

    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        if destructive {
            self.read_only = false;
            self.supports_concurrent_execution = false;
            self.requires_approval = true;
        }
        self
    }

    pub fn workspace_scope(mut self, scope: WorkspaceScope) -> Self {
        self.workspace_scope = scope;
        self
    }

    pub fn supports_concurrent_execution(mut self, supports: bool) -> Self {
        self.supports_concurrent_execution = supports && self.read_only && !self.destructive;
        self
    }

    pub fn requires_approval(mut self, requires: bool) -> Self {
        self.requires_approval = requires || self.destructive;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservableToolInput {
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

impl ObservableToolInput {
    pub fn new(tool_name: impl Into<String>, arguments: serde_json::Value) -> Self {
        Self {
            tool_name: tool_name.into(),
            arguments: canonicalize_value(arguments),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolReplayKey {
    pub tool_name: String,
    pub arguments_hash: String,
}

impl ToolReplayKey {
    pub fn from_observable(input: &ObservableToolInput) -> Self {
        let payload = serde_json::to_string(&input.arguments).unwrap_or_default();
        Self {
            tool_name: input.tool_name.clone(),
            arguments_hash: stable_hash(&payload),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultSummary {
    pub status: ToolStatus,
    pub preview: String,
}

impl ToolResultSummary {
    pub fn from_output(output: &ToolOutput, max_chars: usize) -> Self {
        let raw = match (&output.data, &output.error) {
            (Some(data), _) => serde_json::to_string(data).unwrap_or_else(|_| format!("{data:?}")),
            (None, Some(error)) => error.clone(),
            (None, None) => String::new(),
        };
        Self {
            status: output.status,
            preview: truncate_chars(&raw, max_chars),
        }
    }
}

fn infer_effect(name: &str) -> ToolEffect {
    match name {
        "read" | "read_file" | "list_dir" | "grep" | "glob" | "search_files" | "system_info"
        | "self_test" => ToolEffect::Read,
        "fs" | "write_file" | "edit" | "memory" => ToolEffect::Write,
        "shell" => ToolEffect::Process,
        "webfetch" | "search" | "web_search" => ToolEffect::Network,
        "browser" | "browser_automation" | "browser_session" | "screenshot" => ToolEffect::Browser,
        "automation" => ToolEffect::System,
        _ => ToolEffect::Unknown,
    }
}

fn infer_read_only(name: &str, effect: ToolEffect) -> bool {
    matches!(
        name,
        "read"
            | "grep"
            | "glob"
            | "search"
            | "web_search"
            | "search_files"
            | "webfetch"
            | "read_file"
            | "list_dir"
            | "screenshot"
            | "system_info"
            | "self_test"
    ) || matches!(effect, ToolEffect::Read)
}

fn infer_destructive(name: &str, effect: ToolEffect) -> bool {
    matches!(name, "shell" | "automation" | "write_file" | "edit" | "fs")
        || matches!(effect, ToolEffect::System)
}

fn infer_workspace_scope(name: &str, effect: ToolEffect) -> WorkspaceScope {
    match (name, effect) {
        (
            "read" | "read_file" | "list_dir" | "grep" | "glob" | "search_files" | "fs"
            | "write_file" | "edit",
            _,
        ) => WorkspaceScope::WorkspaceOnly,
        ("shell", _) => WorkspaceScope::CurrentDirectoryOnly,
        (_, ToolEffect::Network | ToolEffect::Browser) => WorkspaceScope::External,
        _ => WorkspaceScope::Unrestricted,
    }
}

fn canonicalize_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(canonicalize_value).collect())
        }
        serde_json::Value::Object(map) => {
            let mut sorted = serde_json::Map::new();
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            for (key, value) in entries {
                sorted.insert(key, canonicalize_value(value));
            }
            serde_json::Value::Object(sorted)
        }
        other => other,
    }
}

fn stable_hash(text: &str) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in text.chars().take(max_chars) {
        out.push(ch);
    }
    if text.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct DummyTool {
        name: &'static str,
    }

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "dummy"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
            ToolOutput::success(serde_json::json!({"ok": true}))
        }
    }

    #[test]
    fn default_spec_infers_read_only_tools() {
        let spec = ToolSpec::from_tool(&DummyTool {
            name: "search_files",
        });

        assert_eq!(spec.effect, ToolEffect::Read);
        assert!(spec.read_only);
        assert!(spec.supports_concurrent_execution);
        assert!(!spec.requires_approval);
        assert_eq!(spec.workspace_scope, WorkspaceScope::WorkspaceOnly);
    }

    #[test]
    fn default_spec_treats_shell_as_process_with_approval() {
        let spec = ToolSpec::from_tool(&DummyTool { name: "shell" });

        assert_eq!(spec.effect, ToolEffect::Process);
        assert!(!spec.read_only);
        assert!(spec.destructive);
        assert!(spec.requires_approval);
        assert_eq!(spec.workspace_scope, WorkspaceScope::CurrentDirectoryOnly);
    }

    #[test]
    fn observable_input_canonicalizes_argument_order() {
        let left = ObservableToolInput::new("tool", serde_json::json!({"b": 1, "a": 2}));
        let right = ObservableToolInput::new("tool", serde_json::json!({"a": 2, "b": 1}));

        assert_eq!(left, right);
        assert_eq!(
            ToolReplayKey::from_observable(&left),
            ToolReplayKey::from_observable(&right)
        );
    }

    #[test]
    fn result_summary_truncates_preview() {
        let summary = ToolResultSummary::from_output(
            &ToolOutput::success(serde_json::json!({"value": "abcdef"})),
            8,
        );

        assert_eq!(summary.status, ToolStatus::Success);
        assert!(summary.preview.chars().count() <= 11);
    }
}
