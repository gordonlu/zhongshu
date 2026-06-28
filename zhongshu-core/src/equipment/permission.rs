use std::sync::Arc;

use crate::agent::llm::ToolDef;
use crate::tool::{Tool, ToolOutput};
use async_trait::async_trait;

use super::manifest::EquipmentPermissions;

/// Wraps a tool with equipment permission checks.
/// Intercepts execute() to verify the requested action is allowed
/// by the equipment's declared permissions.
pub struct PermissionGuard {
    inner: Arc<dyn Tool>,
    permissions: EquipmentPermissions,
}

impl PermissionGuard {
    pub fn new(tool: Arc<dyn Tool>, permissions: EquipmentPermissions) -> Self {
        PermissionGuard {
            inner: tool,
            permissions,
        }
    }

    fn check_shell(&self, args: &serde_json::Value) -> Result<(), String> {
        let sp = &self.permissions.shell;
        if sp.allowed {
            return Ok(());
        }
        // Extract command name from shell arguments.
        let cmd_str = args.as_str().unwrap_or("");
        let cmd = cmd_str.split_whitespace().next().unwrap_or("");
        if sp.allowed_commands.is_empty() {
            return Err("shell access not allowed by equipment permissions".into());
        }
        if !sp.allowed_commands.contains(&cmd.to_string()) {
            return Err(format!(
                "command '{}' not in equipment's allowed list ({:?})",
                cmd, sp.allowed_commands
            ));
        }
        Ok(())
    }

    fn check_network(&self, _args: &serde_json::Value) -> Result<(), String> {
        let np = &self.permissions.network;
        if np.allowed {
            return Ok(());
        }
        Err("network access not allowed by equipment permissions".into())
    }

    fn check_filesystem(&self, _args: &serde_json::Value) -> Result<(), String> {
        let fp = &self.permissions.filesystem;
        if fp.allowed {
            return Ok(());
        }
        Err("filesystem access not allowed by equipment permissions".into())
    }

    fn check_browser(&self, _args: &serde_json::Value) -> Result<(), String> {
        let bp = &self.permissions.browser;
        if bp.allowed {
            return Ok(());
        }
        Err("browser access not allowed by equipment permissions".into())
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
        match self.inner.name() {
            "shell" => {
                if let Err(msg) = self.check_shell(arguments) {
                    return ToolOutput::error(msg);
                }
            }
            "webfetch" | "search" => {
                if let Err(msg) = self.check_network(arguments) {
                    return ToolOutput::error(msg);
                }
            }
            "read" | "grep" | "glob" | "search_files" | "fs" | "list_dir" | "read_file"
            | "write_file" | "edit" => {
                if let Err(msg) = self.check_filesystem(arguments) {
                    return ToolOutput::error(msg);
                }
            }
            "browser" | "browser_automation" | "browser_session" | "screenshot" => {
                if let Err(msg) = self.check_browser(arguments) {
                    return ToolOutput::error(msg);
                }
            }
            _ => {}
        }
        self.inner.execute(arguments).await
    }

    fn to_tool_def(&self) -> ToolDef {
        self.inner.to_tool_def()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolOutput;
    use async_trait::async_trait;
    use serde_json::json;

    struct FakeTool {
        name: &'static str,
    }

    #[async_trait]
    impl Tool for FakeTool {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "fake"
        }
        fn parameters(&self) -> serde_json::Value {
            json!({})
        }
        async fn execute(&self, _args: &serde_json::Value) -> ToolOutput {
            ToolOutput::success(json!({"ok": true}))
        }
    }

    fn run_async<F, T>(f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        tokio::runtime::Runtime::new().unwrap().block_on(f)
    }

    #[test]
    fn shell_allowed_passes() {
        let guard = PermissionGuard::new(
            Arc::new(FakeTool { name: "shell" }),
            EquipmentPermissions {
                shell: super::super::manifest::ShellPermission {
                    allowed: true,
                    allowed_commands: vec![],
                },
                ..Default::default()
            },
        );
        let output = run_async(guard.execute(&json!("cargo build")));
        assert_eq!(output.status, crate::tool::ToolStatus::Success);
    }

    #[test]
    fn shell_not_allowed_fails() {
        let guard = PermissionGuard::new(
            Arc::new(FakeTool { name: "shell" }),
            EquipmentPermissions {
                shell: super::super::manifest::ShellPermission {
                    allowed: false,
                    allowed_commands: vec![],
                },
                ..Default::default()
            },
        );
        let output = run_async(guard.execute(&json!("cargo build")));
        assert_eq!(output.status, crate::tool::ToolStatus::Error);
    }

    #[test]
    fn network_not_allowed_fails() {
        let guard = PermissionGuard::new(
            Arc::new(FakeTool { name: "webfetch" }),
            EquipmentPermissions::default(),
        );
        let output = run_async(guard.execute(&json!({"url": "https://example.com"})));
        assert_eq!(output.status, crate::tool::ToolStatus::Error);
        assert!(output.error.unwrap().contains("network"));
    }

    #[test]
    fn network_allowed_passes() {
        let guard = PermissionGuard::new(
            Arc::new(FakeTool { name: "webfetch" }),
            EquipmentPermissions {
                network: super::super::manifest::NetworkPermission {
                    allowed: true,
                    allowed_hosts: vec![],
                },
                ..Default::default()
            },
        );
        let output = run_async(guard.execute(&json!({"url": "https://example.com"})));
        assert_eq!(output.status, crate::tool::ToolStatus::Success);
    }

    #[test]
    fn filesystem_not_allowed_fails() {
        let guard = PermissionGuard::new(
            Arc::new(FakeTool { name: "read" }),
            EquipmentPermissions::default(),
        );
        let output = run_async(guard.execute(&json!({"path": "/tmp/file.txt"})));
        assert_eq!(output.status, crate::tool::ToolStatus::Error);
        assert!(output.error.unwrap().contains("filesystem"));
    }

    #[test]
    fn browser_not_allowed_fails() {
        let guard = PermissionGuard::new(
            Arc::new(FakeTool { name: "browser" }),
            EquipmentPermissions::default(),
        );
        let output = run_async(guard.execute(&json!({"action": "navigate"})));
        assert_eq!(output.status, crate::tool::ToolStatus::Error);
        assert!(output.error.unwrap().contains("browser"));
    }

    #[test]
    fn unknown_tool_passes_through() {
        let guard = PermissionGuard::new(
            Arc::new(FakeTool { name: "self_test" }),
            EquipmentPermissions::default(),
        );
        let output = run_async(guard.execute(&json!({})));
        assert_eq!(output.status, crate::tool::ToolStatus::Success);
    }
}
