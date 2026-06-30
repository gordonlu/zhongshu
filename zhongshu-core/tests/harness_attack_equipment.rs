//! Equipment and MCP attack tests.
//!
//! These tests avoid spawning real MCP servers. They verify the registry and
//! permission semantics that must hold before equipment-provided capabilities
//! can reach the agent.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use zhongshu_core::equipment::{
    BrowserPermission, EquipmentEntry, EquipmentPermissions, EquipmentRegistry, EquipmentStatus,
    EquipmentType, FilesystemPermission, Manifest, McpServerConfig, McpToolDefinition,
    NetworkPermission, PermissionGuard, ShellPermission,
};
use zhongshu_core::tool::{
    Tool, ToolEffect, ToolOutput, ToolRegistry, ToolSpec, ToolStatus, WorkspaceScope,
};

struct FakeTool {
    name: &'static str,
}

#[async_trait]
impl Tool for FakeTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "fake equipment attack test tool"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({})
    }

    async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
        ToolOutput::success(json!({ "executed": self.name }))
    }
}

struct ApprovalRequiredTool {
    name: &'static str,
}

#[async_trait]
impl Tool for ApprovalRequiredTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "fake MCP-like tool requiring approval"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({})
    }

    async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
        ToolOutput::success(json!({ "executed": self.name }))
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(self.name).requires_approval(true)
    }
}

fn manifest(name: &str, equipment_type: EquipmentType) -> Manifest {
    Manifest {
        name: name.to_string(),
        version: "1.0.0".to_string(),
        description: String::new(),
        equipment_type,
        tools: Vec::new(),
        profiles: Vec::new(),
        mcp_servers: Vec::new(),
        permissions: EquipmentPermissions::default(),
        entry: EquipmentEntry::None {},
    }
}

fn write_equipment(base: &Path, manifest: &Manifest, prompt: Option<&str>) {
    let dir = base.join(&manifest.name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(manifest).unwrap(),
    )
    .unwrap();
    if let Some(prompt) = prompt {
        fs::write(dir.join("prompt.md"), prompt).unwrap();
    }
}

#[test]
fn constrained_permissions_and_mcp_servers_require_approval() {
    let mut network = manifest("host-bound-network", EquipmentType::ToolExtension);
    network.permissions.network = NetworkPermission {
        allowed: false,
        allowed_hosts: vec!["example.com".to_string()],
    };
    assert!(EquipmentRegistry::needs_approval(&network));

    let mut filesystem = manifest("workspace-bound-fs", EquipmentType::ToolExtension);
    filesystem.permissions.filesystem = FilesystemPermission {
        allowed: false,
        allowed_paths: vec!["D:/zhongshu".to_string()],
        write_allowed: false,
    };
    assert!(EquipmentRegistry::needs_approval(&filesystem));

    let mut writer = manifest("workspace-writer", EquipmentType::ToolExtension);
    writer.permissions.filesystem = FilesystemPermission {
        allowed: false,
        allowed_paths: Vec::new(),
        write_allowed: true,
    };
    assert!(EquipmentRegistry::needs_approval(&writer));

    let mut mcp = manifest("mcp-provider", EquipmentType::ToolExtension);
    mcp.mcp_servers.push(McpServerConfig {
        id: "repo-tools".to_string(),
        command: "node".to_string(),
        args: vec!["server.js".to_string()],
        env: Default::default(),
        working_dir: None,
        timeout_ms: 1_000,
    });
    assert!(EquipmentRegistry::needs_approval(&mcp));
}

#[test]
fn disabled_equipment_does_not_contribute_tools_or_skill_prompts() {
    let base = tempfile::tempdir().unwrap();

    let skill = manifest("disabled-skill", EquipmentType::Skill);
    write_equipment(base.path(), &skill, Some("do not load me"));

    let mut extension = manifest("disabled-extension", EquipmentType::ToolExtension);
    extension.tools = vec!["read_file".to_string()];
    extension.permissions.filesystem = FilesystemPermission {
        allowed: true,
        allowed_paths: Vec::new(),
        write_allowed: false,
    };
    write_equipment(base.path(), &extension, None);

    let mut registry = EquipmentRegistry::new(base.path().to_path_buf());
    registry.scan();
    registry
        .set_status("disabled-skill", EquipmentStatus::Disabled)
        .unwrap();
    registry
        .set_status("disabled-extension", EquipmentStatus::Disabled)
        .unwrap();

    let mut tools = ToolRegistry::new();
    tools.register_ref(Arc::new(FakeTool { name: "read_file" }));

    assert!(registry.skill_prompts().is_empty());
    assert!(registry.equipment_tools(&tools).is_empty());
}

#[tokio::test]
async fn network_permissions_are_enforced_by_host() {
    let guarded = PermissionGuard::new(
        Arc::new(FakeTool { name: "webfetch" }),
        EquipmentPermissions {
            network: NetworkPermission {
                allowed: false,
                allowed_hosts: vec!["example.com".to_string()],
            },
            ..Default::default()
        },
    );

    let allowed = guarded
        .execute(&json!({ "url": "https://docs.example.com/guide" }))
        .await;
    assert_eq!(allowed.status, ToolStatus::Success);

    let denied = guarded
        .execute(&json!({ "url": "https://example.net/guide" }))
        .await;
    assert_eq!(denied.status, ToolStatus::Error);
    assert!(denied.error.unwrap().contains("allowed list"));
}

#[tokio::test]
async fn web_search_uses_network_permissions() {
    let guarded = PermissionGuard::new(
        Arc::new(FakeTool { name: "web_search" }),
        EquipmentPermissions::default(),
    );

    let denied = guarded.execute(&json!({ "query": "zhongshu" })).await;

    assert_eq!(denied.status, ToolStatus::Error);
    assert!(denied.error.unwrap().contains("network"));
}

#[tokio::test]
async fn filesystem_permissions_are_enforced_by_path_and_write_flag() {
    let allowed_dir = tempfile::tempdir().unwrap();
    let allowed_file = allowed_dir.path().join("allowed.txt");
    let outside_dir = tempfile::tempdir().unwrap();
    let outside_file = outside_dir.path().join("outside.txt");

    let read_guard = PermissionGuard::new(
        Arc::new(FakeTool { name: "read_file" }),
        EquipmentPermissions {
            filesystem: FilesystemPermission {
                allowed: false,
                allowed_paths: vec![allowed_dir.path().display().to_string()],
                write_allowed: false,
            },
            ..Default::default()
        },
    );

    let allowed = read_guard
        .execute(&json!({ "path": allowed_file.display().to_string() }))
        .await;
    assert_eq!(allowed.status, ToolStatus::Success);

    let denied = read_guard
        .execute(&json!({ "path": outside_file.display().to_string() }))
        .await;
    assert_eq!(denied.status, ToolStatus::Error);
    assert!(denied.error.unwrap().contains("allowed paths"));

    let write_guard = PermissionGuard::new(
        Arc::new(FakeTool { name: "write_file" }),
        EquipmentPermissions {
            filesystem: FilesystemPermission {
                allowed: false,
                allowed_paths: vec![allowed_dir.path().display().to_string()],
                write_allowed: false,
            },
            ..Default::default()
        },
    );
    let denied_write = write_guard
        .execute(&json!({
            "path": allowed_file.display().to_string(),
            "content": "blocked"
        }))
        .await;
    assert_eq!(denied_write.status, ToolStatus::Error);
    assert!(denied_write.error.unwrap().contains("write access"));
}

#[tokio::test]
async fn mcp_like_tools_require_declared_equipment_permissions() {
    let unpermitted = PermissionGuard::new(
        Arc::new(ApprovalRequiredTool {
            name: "mcp_repo_write",
        }),
        EquipmentPermissions::default(),
    );

    let denied = unpermitted.execute(&json!({ "path": "repo.txt" })).await;
    assert_eq!(denied.status, ToolStatus::Error);
    assert!(denied.error.unwrap().contains("declared permissions"));

    let allowed_dir = tempfile::tempdir().unwrap();
    let permitted = PermissionGuard::new(
        Arc::new(ApprovalRequiredTool {
            name: "mcp_repo_write",
        }),
        EquipmentPermissions {
            filesystem: FilesystemPermission {
                allowed: false,
                allowed_paths: vec![allowed_dir.path().display().to_string()],
                write_allowed: true,
            },
            ..Default::default()
        },
    );

    let allowed = permitted
        .execute(&json!({ "path": allowed_dir.path().join("repo.txt").display().to_string() }))
        .await;
    assert_eq!(allowed.status, ToolStatus::Success);

    let outside_dir = tempfile::tempdir().unwrap();
    let denied_outside = permitted
        .execute(&json!({ "path": outside_dir.path().join("repo.txt").display().to_string() }))
        .await;
    assert_eq!(denied_outside.status, ToolStatus::Error);
    assert!(denied_outside.error.unwrap().contains("allowed paths"));
}

#[test]
fn static_tool_specs_do_not_understate_equipment_sensitive_tools() {
    let web_search = ToolSpec::from_tool(&FakeTool { name: "web_search" });
    assert_eq!(web_search.effect, ToolEffect::Network);
    assert!(web_search.read_only);
    assert_eq!(web_search.workspace_scope, WorkspaceScope::External);

    let read_file = ToolSpec::from_tool(&FakeTool { name: "read_file" });
    assert_eq!(read_file.effect, ToolEffect::Read);
    assert!(read_file.read_only);
    assert_eq!(read_file.workspace_scope, WorkspaceScope::WorkspaceOnly);

    let write_file = ToolSpec::from_tool(&FakeTool { name: "write_file" });
    assert_eq!(write_file.effect, ToolEffect::Write);
    assert!(write_file.destructive);
    assert!(write_file.requires_approval);

    let mcp = McpToolDefinition {
        server_id: "repo-tools".to_string(),
        name: "repo_write".to_string(),
        description: "write to repo".to_string(),
        input_schema: json!({ "type": "object" }),
    }
    .spec();
    assert_eq!(mcp.effect, ToolEffect::Unknown);
    assert_eq!(mcp.workspace_scope, WorkspaceScope::External);
    assert!(mcp.requires_approval);

    let browser = manifest("browser-equipment", EquipmentType::ToolExtension);
    let browser_permissions = EquipmentPermissions {
        browser: BrowserPermission { allowed: true },
        shell: ShellPermission::default(),
        network: NetworkPermission::default(),
        filesystem: FilesystemPermission::default(),
    };
    assert!(!EquipmentRegistry::needs_approval(&browser));
    let mut browser = browser;
    browser.permissions = browser_permissions;
    assert!(EquipmentRegistry::needs_approval(&browser));
}
