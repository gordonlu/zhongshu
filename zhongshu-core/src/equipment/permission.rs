use std::sync::Arc;

use async_trait::async_trait;
use crate::agent::llm::ToolDef;
use crate::tool::{Tool, ToolOutput};

use super::manifest::EquipmentPermissions;

/// Wraps a tool with equipment permission checks.
/// Intercepts `execute()` to verify the requested action is allowed
/// by the equipment's declared permissions.
pub struct PermissionGuard {
    inner: Arc<dyn Tool>,
    permissions: EquipmentPermissions,
}

impl PermissionGuard {
    pub fn new(tool: Arc<dyn Tool>, permissions: EquipmentPermissions) -> Self {
        PermissionGuard { inner: tool, permissions }
    }

    fn check_shell(&self, args: &serde_json::Value) -> Result<(), String> {
        let sp = &self.permissions.shell;
        if sp.allowed {
            return Ok(());
        }
        // Extract command name from shell arguments.
        let cmd_str = args.as_str().unwrap_or("");
        let cmd = cmd_str.split_whitespace().next().unwrap_or("");
        if !sp.allowed_commands.is_empty() && !sp.allowed_commands.contains(&cmd.to_string()) {
            return Err(format!(
                "command '{}' not in equipment's allowed list ({:?})",
                cmd, sp.allowed_commands
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl Tool for PermissionGuard {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters(&self) -> serde_json::Value {
        self.inner.parameters()
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        // Shell permission check
        if self.inner.name() == "shell" {
            if let Err(msg) = self.check_shell(arguments) {
                return ToolOutput::error(msg);
            }
        }
        self.inner.execute(arguments).await
    }

    fn to_tool_def(&self) -> ToolDef {
        self.inner.to_tool_def()
    }
}
