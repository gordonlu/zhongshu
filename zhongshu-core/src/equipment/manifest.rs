use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Unique equipment identifier (derived from name).
pub type EquipmentId = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum EquipmentType {
    /// Stable reusable skill (trigger + flow).
    Skill,
    /// New system capability (tool extension).
    ToolExtension,
    /// Multi-step execution template.
    Workflow,
    /// Specialised agent profile (installs to profiles/).
    WorkerProfile,
}

impl Default for EquipmentType {
    fn default() -> Self {
        EquipmentType::Skill
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum EquipmentStatus {
    Active,
    Disabled,
    Failed,
}

impl Default for EquipmentStatus {
    fn default() -> Self {
        EquipmentStatus::Active
    }
}

/// Permissions declared by an equipment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquipmentPermissions {
    #[serde(default)]
    pub shell: ShellPermission,
    /// Network access permissions (webfetch, search, etc.).
    #[serde(default)]
    pub network: NetworkPermission,
    /// Filesystem access permissions (read, write, fs, etc.).
    #[serde(default)]
    pub filesystem: FilesystemPermission,
    /// Browser automation permissions.
    #[serde(default)]
    pub browser: BrowserPermission,
}

impl Default for EquipmentPermissions {
    fn default() -> Self {
        EquipmentPermissions {
            shell: ShellPermission::default(),
            network: NetworkPermission::default(),
            filesystem: FilesystemPermission::default(),
            browser: BrowserPermission::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellPermission {
    /// If true, any shell command is allowed.
    #[serde(default)]
    pub allowed: bool,
    /// If non-empty, only commands whose first word is in this list.
    #[serde(default)]
    pub allowed_commands: Vec<String>,
}

impl Default for ShellPermission {
    fn default() -> Self {
        ShellPermission {
            allowed: false,
            allowed_commands: Vec::new(),
        }
    }
}

/// Network access permission (webfetch, search, MCP server connections).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPermission {
    /// If true, any network access is allowed.
    #[serde(default)]
    pub allowed: bool,
    /// If non-empty, only these hostnames/domains are reachable.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

impl Default for NetworkPermission {
    fn default() -> Self {
        NetworkPermission {
            allowed: false,
            allowed_hosts: Vec::new(),
        }
    }
}

/// Filesystem access permission (read, write, fs tools).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemPermission {
    /// If true, any filesystem access is allowed.
    #[serde(default)]
    pub allowed: bool,
    /// If non-empty, only paths under these directories are accessible.
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    /// If true, write operations are allowed (read-only otherwise).
    #[serde(default)]
    pub write_allowed: bool,
}

impl Default for FilesystemPermission {
    fn default() -> Self {
        FilesystemPermission {
            allowed: false,
            allowed_paths: Vec::new(),
            write_allowed: false,
        }
    }
}

/// Browser automation permission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserPermission {
    /// If true, browser automation is allowed.
    #[serde(default)]
    pub allowed: bool,
}

impl Default for BrowserPermission {
    fn default() -> Self {
        BrowserPermission { allowed: false }
    }
}

/// Entry point of an equipment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EquipmentEntry {
    /// Path to a workflow file, relative to equipment dir.
    Workflow(String),
    /// Path to a prompt file, relative to equipment dir.
    Prompt(String),
    /// For ToolExtension: no entry, tool code is inline.
    None {},
}

impl Default for EquipmentEntry {
    fn default() -> Self {
        EquipmentEntry::None {}
    }
}

/// The manifest.json file inside an equipment package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    #[serde(rename = "type")]
    pub equipment_type: EquipmentType,
    /// Tools this equipment uses (by name, e.g. ["shell", "git"]).
    #[serde(default)]
    pub tools: Vec<String>,
    /// Worker profiles this equipment provides (paths relative to equipment dir).
    #[serde(default)]
    pub profiles: Vec<String>,
    /// Stdio MCP servers this equipment provides.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    pub permissions: EquipmentPermissions,
    #[serde(default)]
    pub entry: EquipmentEntry,
}

impl Manifest {
    pub fn id(&self) -> EquipmentId {
        self.name.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerConfig {
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default = "default_mcp_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_mcp_timeout_ms() -> u64 {
    10_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrip() {
        let m = Manifest {
            name: "rust-release".into(),
            version: "1.0.0".into(),
            description: "Rust release workflow".into(),
            equipment_type: EquipmentType::Workflow,
            tools: vec!["shell".into(), "git".into()],
            profiles: vec![],
            mcp_servers: vec![],
            permissions: EquipmentPermissions {
                shell: ShellPermission {
                    allowed: false,
                    allowed_commands: vec!["cargo".into(), "git".into()],
                },
                ..Default::default()
            },
            entry: EquipmentEntry::Workflow("workflow.yaml".into()),
        };
        let json = serde_json::to_string_pretty(&m).unwrap();
        let parsed: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "rust-release");
        assert_eq!(parsed.equipment_type, EquipmentType::Workflow);
        assert_eq!(
            parsed.permissions.shell.allowed_commands,
            vec!["cargo", "git"]
        );
    }

    #[test]
    fn manifest_defaults() {
        let json = r#"{"name":"test","version":"0.1.0"}"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.equipment_type, EquipmentType::Skill);
        assert!(m.tools.is_empty());
        assert!(m.mcp_servers.is_empty());
        assert!(!m.permissions.shell.allowed);
    }

    #[test]
    fn manifest_mcp_server_roundtrip() {
        let json = r#"{
            "name": "mcp-ext",
            "version": "1.0.0",
            "type": "tool-extension",
            "mcp_servers": [{
                "id": "repo-tools",
                "command": "node",
                "args": ["server.js"],
                "env": {"A": "B"},
                "working_dir": ".",
                "timeout_ms": 2500
            }]
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();

        assert_eq!(m.mcp_servers.len(), 1);
        assert_eq!(m.mcp_servers[0].id, "repo-tools");
        assert_eq!(m.mcp_servers[0].args, vec!["server.js"]);
        assert_eq!(m.mcp_servers[0].env.get("A").map(String::as_str), Some("B"));
        assert_eq!(m.mcp_servers[0].timeout_ms, 2500);
    }

    #[test]
    fn manifest_network_permission_roundtrip() {
        let json = r#"{
            "name": "web-ext",
            "version": "1.0.0",
            "type": "tool-extension",
            "permissions": {
                "network": { "allowed": true, "allowed_hosts": ["api.example.com"] }
            }
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert!(m.permissions.network.allowed);
        assert_eq!(m.permissions.network.allowed_hosts, vec!["api.example.com"]);
    }

    #[test]
    fn manifest_filesystem_permission_roundtrip() {
        let json = r#"{
            "name": "fs-ext",
            "version": "1.0.0",
            "permissions": {
                "filesystem": { "allowed": true, "allowed_paths": ["/tmp/work"], "write_allowed": true }
            }
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert!(m.permissions.filesystem.allowed);
        assert!(m.permissions.filesystem.write_allowed);
        assert_eq!(m.permissions.filesystem.allowed_paths, vec!["/tmp/work"]);
    }

    #[test]
    fn manifest_browser_permission_roundtrip() {
        let json = r#"{
            "name": "browser-ext",
            "version": "1.0.0",
            "permissions": {
                "browser": { "allowed": true }
            }
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert!(m.permissions.browser.allowed);
    }

    #[test]
    fn manifest_all_permissions_default_to_false() {
        let json = r#"{"name":"minimal","version":"0.1.0"}"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert!(!m.permissions.shell.allowed);
        assert!(!m.permissions.network.allowed);
        assert!(!m.permissions.filesystem.allowed);
        assert!(!m.permissions.browser.allowed);
    }
}
