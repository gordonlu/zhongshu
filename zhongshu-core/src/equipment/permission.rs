use std::sync::Arc;

use crate::agent::llm::ToolDef;
use crate::tool::{Tool, ToolEffect, ToolOutput};
use async_trait::async_trait;
use std::path::{Component, Path, PathBuf};

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

    fn check_network(&self, tool_name: &str, args: &serde_json::Value) -> Result<(), String> {
        let np = &self.permissions.network;
        if np.allowed && np.allowed_hosts.is_empty() {
            return Ok(());
        }

        if np.allowed_hosts.is_empty() {
            return Err("network access not allowed by equipment permissions".into());
        }

        let host = match network_host_for_tool(tool_name, args) {
            Some(host) => host,
            None => {
                return Err(
                    "network host could not be determined for equipment permission check".into(),
                )
            }
        };

        if host_allowed(&host, &np.allowed_hosts) {
            Ok(())
        } else {
            Err(format!(
                "network host '{}' not in equipment's allowed list ({:?})",
                host, np.allowed_hosts
            ))
        }
    }

    fn check_filesystem(&self, tool_name: &str, args: &serde_json::Value) -> Result<(), String> {
        let fp = &self.permissions.filesystem;
        if is_write_tool(tool_name) && !fp.write_allowed {
            return Err("filesystem write access not allowed by equipment permissions".into());
        }

        if fp.allowed && fp.allowed_paths.is_empty() {
            return Ok(());
        }

        if fp.allowed_paths.is_empty() {
            return Err("filesystem access not allowed by equipment permissions".into());
        }

        let requested_path = args["path"]
            .as_str()
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| {
                "filesystem path could not be determined for equipment permission check".to_string()
            })?;

        if path_allowed(requested_path, &fp.allowed_paths) {
            Ok(())
        } else {
            Err(format!(
                "filesystem path '{}' not under equipment's allowed paths ({:?})",
                requested_path, fp.allowed_paths
            ))
        }
    }

    fn check_browser(&self, _args: &serde_json::Value) -> Result<(), String> {
        let bp = &self.permissions.browser;
        if bp.allowed {
            return Ok(());
        }
        Err("browser access not allowed by equipment permissions".into())
    }

    fn check_unclassified_tool(&self, args: &serde_json::Value) -> Result<(), String> {
        let spec = self.inner.spec();
        if spec.read_only && !spec.requires_approval && matches!(spec.effect, ToolEffect::Read) {
            return Ok(());
        }
        if declares_external_permission(&self.permissions) {
            if args.get("url").is_some() {
                self.check_network("webfetch", args)?;
            }
            if args.get("path").is_some() {
                let filesystem_tool = if args.get("content").is_some()
                    || args.get("old").is_some()
                    || args.get("new").is_some()
                {
                    "write_file"
                } else {
                    "read_file"
                };
                self.check_filesystem(filesystem_tool, args)?;
            }
            if let Some(command) = args.get("command") {
                self.check_shell(command)?;
            }
            if args.get("action").is_some() {
                self.check_browser(args)?;
            }
            return Ok(());
        }
        Err(format!(
            "equipment tool '{}' is not covered by declared permissions",
            self.inner.name()
        ))
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
            "webfetch" | "search" | "web_search" => {
                if let Err(msg) = self.check_network(self.inner.name(), arguments) {
                    return ToolOutput::error(msg);
                }
            }
            "read" | "grep" | "glob" | "search_files" | "fs" | "list_dir" | "read_file"
            | "write_file" | "edit" => {
                if let Err(msg) = self.check_filesystem(self.inner.name(), arguments) {
                    return ToolOutput::error(msg);
                }
            }
            "browser" | "browser_automation" | "browser_session" | "screenshot" => {
                if let Err(msg) = self.check_browser(arguments) {
                    return ToolOutput::error(msg);
                }
            }
            _ => {
                if let Err(msg) = self.check_unclassified_tool(arguments) {
                    return ToolOutput::error(msg);
                }
            }
        }
        self.inner.execute(arguments).await
    }

    fn to_tool_def(&self) -> ToolDef {
        self.inner.to_tool_def()
    }
}

fn network_host_for_tool(tool_name: &str, args: &serde_json::Value) -> Option<String> {
    match tool_name {
        "webfetch" => args["url"]
            .as_str()
            .and_then(|url| reqwest::Url::parse(url).ok())
            .and_then(|url| url.host_str().map(|host| host.to_ascii_lowercase())),
        "search" | "web_search" => Some("html.duckduckgo.com".to_string()),
        _ => None,
    }
}

fn host_allowed(host: &str, allowed_hosts: &[String]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    allowed_hosts.iter().any(|allowed| {
        let allowed = allowed.trim().trim_end_matches('.').to_ascii_lowercase();
        !allowed.is_empty() && (host == allowed || host.ends_with(&format!(".{allowed}")))
    })
}

fn is_write_tool(tool_name: &str) -> bool {
    matches!(tool_name, "fs" | "write_file" | "edit")
}

fn declares_external_permission(permissions: &EquipmentPermissions) -> bool {
    permissions.shell.allowed
        || !permissions.shell.allowed_commands.is_empty()
        || permissions.network.allowed
        || !permissions.network.allowed_hosts.is_empty()
        || permissions.filesystem.allowed
        || !permissions.filesystem.allowed_paths.is_empty()
        || permissions.filesystem.write_allowed
        || permissions.browser.allowed
}

fn path_allowed(requested_path: &str, allowed_paths: &[String]) -> bool {
    let requested = resolve_path_for_permission(Path::new(requested_path));
    allowed_paths.iter().any(|allowed| {
        let allowed = resolve_path_for_permission(Path::new(allowed));
        requested == allowed || requested.starts_with(&allowed)
    })
}

fn resolve_path_for_permission(path: &Path) -> PathBuf {
    let normalized = normalize_path(path);
    if let Ok(canonical) = std::fs::canonicalize(&normalized) {
        return canonical;
    }

    let mut cursor = normalized.as_path();
    let mut suffix = PathBuf::new();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(cursor) {
            return canonical.join(suffix);
        }
        if let Some(name) = cursor.file_name() {
            let mut next_suffix = PathBuf::from(name);
            next_suffix.push(&suffix);
            suffix = next_suffix;
        }
        let Some(parent) = cursor.parent() else {
            break;
        };
        cursor = parent;
    }

    normalized
}

fn normalize_path(path: &Path) -> PathBuf {
    let base = if path.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };
    let mut normalized = base;
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
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
