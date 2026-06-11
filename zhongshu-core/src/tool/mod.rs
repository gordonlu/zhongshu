pub mod automation;
pub mod browser;
pub mod fs;
pub mod screenshot;
pub mod search;
pub mod shell;
pub mod system_info;
pub mod webfetch;

use crate::agent::llm::{ToolDef, ToolFunctionDef};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub status: ToolStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_program: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_command: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Success,
    Error,
    AuthRequired,
}

impl ToolOutput {
    pub fn success(data: serde_json::Value) -> Self {
        ToolOutput { status: ToolStatus::Success, data: Some(data), error: None, auth_program: None, auth_command: None }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        ToolOutput { status: ToolStatus::Error, data: None, error: Some(msg.into()), auth_program: None, auth_command: None }
    }

    pub fn auth_required(program: &str, command: &str) -> Self {
        ToolOutput {
            status: ToolStatus::AuthRequired,
            data: None,
            error: Some(format!("Command '{}' requires approval", program)),
            auth_program: Some(program.to_string()),
            auth_command: Some(command.to_string()),
        }
    }

    pub fn is_auth_required(&self) -> bool {
        self.status == ToolStatus::AuthRequired
    }

    pub fn render_observation(&self, tool_name: &str) -> String {
        let mut lines = vec![format!("<observation tool=\"{tool_name}\" status=\"{}\">", self.status_str())];
        if let Some(ref data) = self.data {
            lines.push(serde_json::to_string_pretty(data).unwrap_or_else(|_| format!("{data:?}")));
        }
        if let Some(ref err) = self.error {
            lines.push(format!("error: {err}"));
        }
        lines.push("</observation>".to_string());
        lines.join("\n")
    }

    fn status_str(&self) -> &str {
        match self.status {
            ToolStatus::Success => "success",
            ToolStatus::Error => "error",
            ToolStatus::AuthRequired => "auth_required",
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;
    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput;

    fn to_tool_def(&self) -> ToolDef {
        ToolDef {
            def_type: "function".into(),
            function: ToolFunctionDef {
                name: self.name().into(),
                description: self.description().into(),
                parameters: self.parameters(),
            },
        }
    }
}

#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        ToolRegistry { tools: HashMap::new() }
    }

    pub fn register(mut self, tool: impl Tool + 'static) -> Self {
        self.tools.insert(tool.name().to_string(), Arc::new(tool));
        self
    }

    /// Register a tool at runtime (equipment installation).
    pub fn register_ref(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    pub async fn execute(&self, name: &str, arguments: &str) -> ToolOutput {
        let args: serde_json::Value = if arguments.trim().is_empty() {
            serde_json::Value::Object(serde_json::Map::new())
        } else {
            match serde_json::from_str(arguments) {
                Ok(v) => v,
                Err(e) => return ToolOutput::error(format!("参数解析失败: {e}")),
            }
        };

        let tool = match self.get(name) {
            Some(t) => t,
            None => return ToolOutput::error(format!("未知工具: {name}")),
        };

        tool.execute(&args).await
    }

    pub fn as_tool_defs(&self) -> Vec<ToolDef> {
        self.tools.values().map(|t| t.to_tool_def()).collect()
    }

    /// Build a sub-registry containing only the named tools.
    /// Tools not in the registry are silently skipped.
    pub fn select(&self, names: &[&str]) -> Self {
        ToolRegistry {
            tools: names.iter()
                .filter_map(|n| self.tools.get(*n).map(|t| (n.to_string(), t.clone())))
                .collect(),
        }
    }
}

pub fn default_registry() -> ToolRegistry {
    ToolRegistry::new()
        .register(shell::ShellTool)
        .register(fs::ReadFileTool)
        .register(fs::WriteFileTool)
        .register(fs::ListDirTool)
        .register(system_info::SystemInfoTool)
}
