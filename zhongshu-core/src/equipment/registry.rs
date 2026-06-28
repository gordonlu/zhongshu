use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::tool::{Tool, ToolRegistry};

use super::manifest::{EquipmentId, EquipmentStatus, Manifest};
use super::mcp::{build_mcp_tools, preflight_stdio_server, McpPreflightReport};
use super::permission::PermissionGuard;

/// Callback to check whether a dangerous equipment action should proceed.
/// Returns `true` if the action is approved.
pub type ApprovalCallback = Box<dyn Fn(&Manifest) -> bool + Send + Sync>;

/// An installed equipment package.
#[derive(Debug, Clone)]
pub struct Equipment {
    pub id: EquipmentId,
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub status: EquipmentStatus,
}

/// Scans and manages installed equipment on disk.
pub struct EquipmentRegistry {
    /// Path to the equipment directory (e.g. ~/.config/zhongshu/equipment/).
    base_dir: PathBuf,
    loaded: HashMap<EquipmentId, Equipment>,
    /// Optional callback for dangerous-action approval (install/enable).
    approval_cb: Option<ApprovalCallback>,
}

impl EquipmentRegistry {
    pub fn new(base_dir: PathBuf) -> Self {
        EquipmentRegistry {
            base_dir,
            loaded: HashMap::new(),
            approval_cb: None,
        }
    }

    /// Set a callback for approving dangerous equipment actions (install/enable).
    pub fn set_approval_callback(&mut self, cb: ApprovalCallback) {
        self.approval_cb = Some(cb);
    }

    /// Returns true if the manifest declares capabilities that require approval.
    pub fn needs_approval(manifest: &Manifest) -> bool {
        manifest.permissions.shell.allowed
            || !manifest.permissions.shell.allowed_commands.is_empty()
            || manifest.permissions.network.allowed
            || manifest.permissions.filesystem.allowed
            || manifest.permissions.browser.allowed
            || !manifest.mcp_servers.is_empty()
    }

    /// Scan the equipment directory and load all manifests.
    pub fn scan(&mut self) {
        self.loaded.clear();
        if !self.base_dir.exists() {
            return;
        }
        let entries = match std::fs::read_dir(&self.base_dir) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("equipment: cannot scan {}: {e}", self.base_dir.display());
                return;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let manifest_path = path.join("manifest.json");
            if !manifest_path.exists() {
                continue;
            }
            match std::fs::read_to_string(&manifest_path) {
                Ok(text) => match serde_json::from_str::<Manifest>(&text) {
                    Ok(manifest) => {
                        let id = manifest.id();
                        tracing::info!("equipment: loaded '{}' v{}", id, manifest.version);
                        self.loaded.insert(
                            id.clone(),
                            Equipment {
                                id,
                                manifest,
                                dir: path,
                                status: EquipmentStatus::Active,
                            },
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "equipment: invalid manifest at {}: {e}",
                            manifest_path.display()
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!("equipment: cannot read {}: {e}", manifest_path.display());
                }
            }
        }
    }

    pub fn list(&self) -> Vec<&Equipment> {
        self.loaded.values().collect()
    }

    pub fn get(&self, id: &str) -> Option<&Equipment> {
        self.loaded.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Equipment> {
        self.loaded.get_mut(id)
    }

    /// Iterate all worker profiles provided by active equipment.
    /// Returns (equipment_id, profile_name, profile_path).
    pub fn worker_profiles(&self) -> Vec<(EquipmentId, String, PathBuf)> {
        let mut profiles = Vec::new();
        for eq in self.loaded.values() {
            if eq.status != EquipmentStatus::Active {
                continue;
            }
            for rel_path in &eq.manifest.profiles {
                let full = eq.dir.join(rel_path);
                if full.exists() {
                    let name = full
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| rel_path.clone());
                    profiles.push((eq.id.clone(), name, full));
                }
            }
        }
        profiles
    }

    /// Collect tools from active ToolExtension equipment, wrapped with
    /// PermissionGuard.  Requires a `ToolRegistry` to resolve tool names.
    pub fn equipment_tools(
        &self,
        tool_registry: &ToolRegistry,
    ) -> Vec<(EquipmentId, Arc<dyn Tool>)> {
        let mut result = Vec::new();
        for eq in self.loaded.values() {
            if eq.status != EquipmentStatus::Active {
                continue;
            }
            if !matches!(
                eq.manifest.equipment_type,
                super::manifest::EquipmentType::ToolExtension
            ) {
                continue;
            }
            for tool_name in &eq.manifest.tools {
                if let Some(tool) = tool_registry.get(tool_name) {
                    let guarded = Arc::new(PermissionGuard::new(
                        tool.clone(),
                        eq.manifest.permissions.clone(),
                    )) as Arc<dyn Tool>;
                    result.push((eq.id.clone(), guarded));
                } else {
                    tracing::warn!(
                        "equipment '{}' declares tool '{}' which is not in the registry",
                        eq.id,
                        tool_name
                    );
                }
            }
        }
        result
    }

    /// Register all active ToolExtension equipment's tools (with PermissionGuard)
    /// into the given `ToolRegistry`.
    pub fn register_tools(&self, tool_registry: &mut ToolRegistry) {
        let tools = self.equipment_tools(tool_registry);
        for (eq_id, tool) in &tools {
            tracing::info!(
                "equipment: registering tool '{}' from '{}'",
                tool.name(),
                eq_id
            );
        }
        for (_, tool) in tools {
            tool_registry.register_ref(tool);
        }
    }

    /// Preflight and register active MCP stdio tools.
    ///
    /// A failed MCP server is returned in the report and skipped so one broken
    /// extension cannot poison the whole tool registry.
    pub async fn register_mcp_tools(
        &self,
        tool_registry: &mut ToolRegistry,
    ) -> Vec<McpPreflightReport> {
        let mut reports = Vec::new();
        let servers = self.active_mcp_servers();
        for (eq_id, eq_dir, permissions, server) in servers {
            let report = preflight_stdio_server(&server, &eq_dir).await;
            if report.error.is_none() {
                for tool in build_mcp_tools(&server, &eq_dir, report.tools.clone()) {
                    tracing::info!(
                        "equipment: registering MCP tool '{}' from '{}'",
                        tool.name(),
                        eq_id
                    );
                    let guarded =
                        Arc::new(PermissionGuard::new(tool, permissions.clone())) as Arc<dyn Tool>;
                    tool_registry.register_ref(guarded);
                }
            } else if let Some(error) = &report.error {
                tracing::warn!(
                    "equipment '{}' MCP server '{}' preflight failed: {}",
                    eq_id,
                    report.server_id,
                    error
                );
            }
            reports.push(report);
        }
        reports
    }

    fn active_mcp_servers(
        &self,
    ) -> Vec<(
        EquipmentId,
        PathBuf,
        super::manifest::EquipmentPermissions,
        super::manifest::McpServerConfig,
    )> {
        let mut servers = Vec::new();
        for eq in self.loaded.values() {
            if eq.status != EquipmentStatus::Active {
                continue;
            }
            if !matches!(
                eq.manifest.equipment_type,
                super::manifest::EquipmentType::ToolExtension
            ) {
                continue;
            }
            for server in &eq.manifest.mcp_servers {
                servers.push((
                    eq.id.clone(),
                    eq.dir.clone(),
                    eq.manifest.permissions.clone(),
                    server.clone(),
                ));
            }
        }
        servers
    }

    /// Unregister all tools belonging to a specific equipment from the given
    /// `ToolRegistry`.  Returns the number of tools removed.
    pub fn unregister_tools(&self, tool_registry: &mut ToolRegistry, id: &str) -> usize {
        let Some(eq) = self.loaded.get(id) else {
            return 0;
        };
        let mut count = 0;
        for tool_name in &eq.manifest.tools {
            if tool_registry.unregister(tool_name) {
                count += 1;
            }
        }
        count
    }

    /// Validate that a manifest is well-formed before installation.
    pub fn validate_manifest(manifest: &Manifest) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if manifest.name.trim().is_empty() {
            errors.push("name is required".into());
        }
        if manifest.version.trim().is_empty() {
            errors.push("version is required".into());
        }
        for server in &manifest.mcp_servers {
            if server.id.trim().is_empty() {
                errors.push("mcp server id is required".into());
            }
            if server.command.trim().is_empty() {
                errors.push(format!("mcp server '{}' command is required", server.id));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Install equipment from a source directory into the equipment dir.
    /// If equipment with the same name already exists, upgrades only if
    /// the new version is strictly greater (semver comparison).
    pub fn install_from(&mut self, src: &Path) -> Result<EquipmentId, String> {
        let manifest_path = src.join("manifest.json");
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("cannot read manifest: {e}"))?;
        let manifest: Manifest =
            serde_json::from_str(&text).map_err(|e| format!("invalid manifest: {e}"))?;

        Self::validate_manifest(&manifest).map_err(|errors| errors.join("; "))?;

        // Dangerous equipment requires approval before installation.
        if Self::needs_approval(&manifest) {
            match self.approval_cb {
                Some(ref cb) => {
                    if !cb(&manifest) {
                        return Err(format!(
                            "approval denied for installing '{}'",
                            manifest.name
                        ));
                    }
                }
                None => {
                    return Err(format!(
                        "dangerous equipment '{}' requires approval but no callback is set",
                        manifest.name
                    ));
                }
            }
        }

        let id = manifest.id();
        let dest = self.base_dir.join(&id);

        // Upgrade check: if already installed, only proceed if version > current.
        if let Some(existing) = self.loaded.get(&id) {
            if !is_newer_version(&manifest.version, &existing.manifest.version) {
                return Err(format!(
                    "equipment '{}' v{} already installed (v{} is not newer)",
                    id, existing.manifest.version, manifest.version
                ));
            }
            tracing::info!(
                "equipment: upgrading '{}' v{} → v{}",
                id,
                existing.manifest.version,
                manifest.version
            );
            // Remove old directory before installing new version.
            if existing.dir.exists() {
                std::fs::remove_dir_all(&existing.dir)
                    .map_err(|e| format!("cannot remove old version: {e}"))?;
            }
        }

        // Ensure base dir exists.
        std::fs::create_dir_all(&self.base_dir)
            .map_err(|e| format!("cannot create equipment dir: {e}"))?;

        // Recursively copy.
        copy_dir(src, &dest).map_err(|e| format!("install failed: {e}"))?;

        self.loaded.insert(
            id.clone(),
            Equipment {
                id: id.clone(),
                manifest,
                dir: dest,
                status: EquipmentStatus::Active,
            },
        );

        tracing::info!("equipment: installed '{}'", id);
        Ok(id)
    }

    /// Remove installed equipment (disables + deletes directory).
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        let eq = self
            .loaded
            .remove(id)
            .ok_or_else(|| format!("equipment '{}' not found", id))?;
        // Unregister tools from the registry is caller's responsibility.
        if eq.dir.exists() {
            std::fs::remove_dir_all(&eq.dir)
                .map_err(|e| format!("cannot remove {}: {e}", eq.dir.display()))?;
        }
        tracing::info!("equipment: removed '{}'", id);
        Ok(())
    }

    pub fn set_status(&mut self, id: &str, status: EquipmentStatus) -> Result<(), String> {
        let eq = self
            .loaded
            .get_mut(id)
            .ok_or_else(|| format!("equipment '{}' not found", id))?;

        // Changing from Disabled to Active on dangerous equipment requires approval.
        if status == EquipmentStatus::Active
            && eq.status == EquipmentStatus::Disabled
            && Self::needs_approval(&eq.manifest)
        {
            match self.approval_cb {
                Some(ref cb) => {
                    if !cb(&eq.manifest) {
                        return Err(format!("approval denied for enabling '{}'", id));
                    }
                }
                None => {
                    return Err(format!(
                        "dangerous equipment '{}' requires approval but no callback is set",
                        id
                    ));
                }
            }
        }

        eq.status = status;
        Ok(())
    }

    /// Write built-in equipment to disk if not already installed.
    pub fn install_defaults(&mut self) {
        for (name, _version, manifest_json, prompt_md) in crate::equipment::builtin::all_builtins()
        {
            let dest = self.base_dir.join(name);
            if dest.exists() {
                continue; // already installed, skip
            }
            std::fs::create_dir_all(&dest).unwrap_or_else(|_| {});
            let _ = std::fs::write(dest.join("manifest.json"), manifest_json);
            let _ = std::fs::write(dest.join("prompt.md"), prompt_md);
            tracing::info!("equipment: installed default '{}'", name);
        }
        self.scan(); // reload
    }

    /// Collect skill prompts from all active Skill-type equipment.
    pub fn skill_prompts(&self) -> Vec<(EquipmentId, String)> {
        let mut prompts = Vec::new();
        for eq in self.loaded.values() {
            if eq.status != EquipmentStatus::Active {
                continue;
            }
            if !matches!(
                eq.manifest.equipment_type,
                crate::equipment::EquipmentType::Skill
            ) {
                continue;
            }
            let prompt_path = eq.dir.join("prompt.md");
            if prompt_path.exists() {
                if let Ok(text) = std::fs::read_to_string(&prompt_path) {
                    prompts.push((eq.id.clone(), text));
                }
            }
        }
        prompts
    }
}

/// Recursive directory copy (simple implementation).
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let child_src = entry.path();
            let child_dst = dst.join(entry.file_name());
            if child_src.is_dir() {
                copy_dir(&child_src, &child_dst)?;
            } else {
                std::fs::copy(&child_src, &child_dst)?;
            }
        }
    }
    Ok(())
}

/// Naive semver comparison: "1.2.3" > "1.0.0". Returns true if `new` > `old`.
/// Treats unparseable versions as equal (conservative — don't downgrade by accident).
fn is_newer_version(new: &str, old: &str) -> bool {
    fn parse(v: &str) -> Vec<u32> {
        v.split('.').filter_map(|s| s.parse::<u32>().ok()).collect()
    }
    let a = parse(new);
    let b = parse(old);
    for i in 0..a.len().max(b.len()) {
        let av = a.get(i).copied().unwrap_or(0);
        let bv = b.get(i).copied().unwrap_or(0);
        if av > bv {
            return true;
        }
        if av < bv {
            return false;
        }
    }
    false // equal
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::equipment::manifest::{
        BrowserPermission, EquipmentEntry, EquipmentPermissions, EquipmentType,
        FilesystemPermission, McpServerConfig, NetworkPermission, ShellPermission,
    };
    use std::fs;

    fn make_manifest(
        name: &str,
        eq_type: EquipmentType,
        tools: Vec<&str>,
        shell_allowed: bool,
    ) -> Manifest {
        Manifest {
            name: name.into(),
            version: "1.0.0".into(),
            description: "test".into(),
            equipment_type: eq_type,
            tools: tools.into_iter().map(String::from).collect(),
            profiles: vec![],
            mcp_servers: vec![],
            permissions: EquipmentPermissions {
                shell: ShellPermission {
                    allowed: shell_allowed,
                    allowed_commands: vec![],
                },
                ..Default::default()
            },
            entry: EquipmentEntry::None {},
        }
    }

    fn write_equipment(base: &Path, name: &str, manifest: &Manifest) -> PathBuf {
        let dir = base.join(name);
        fs::create_dir_all(&dir).unwrap();
        let json = serde_json::to_string_pretty(manifest).unwrap();
        fs::write(dir.join("manifest.json"), &json).unwrap();
        dir
    }

    #[test]
    fn scan_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = EquipmentRegistry::new(dir.path().to_path_buf());
        reg.scan();
        assert!(reg.list().is_empty());
    }

    #[test]
    fn install_and_list() {
        let base = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let m = make_manifest("test-tool", EquipmentType::Skill, vec![], false);
        write_equipment(src.path(), "test-tool", &m);

        let mut reg = EquipmentRegistry::new(base.path().to_path_buf());
        let id = reg.install_from(&src.path().join("test-tool")).unwrap();
        assert_eq!(id, "test-tool");

        let mut reg2 = EquipmentRegistry::new(base.path().to_path_buf());
        reg2.scan();
        assert_eq!(reg2.list().len(), 1);
        assert_eq!(reg2.get("test-tool").unwrap().manifest.version, "1.0.0");
    }

    #[test]
    fn validate_manifest_rejects_empty() {
        let m = Manifest {
            name: "".into(),
            version: "".into(),
            description: "".into(),
            equipment_type: EquipmentType::Skill,
            tools: vec![],
            profiles: vec![],
            mcp_servers: vec![],
            permissions: Default::default(),
            entry: EquipmentEntry::None {},
        };
        assert!(EquipmentRegistry::validate_manifest(&m).is_err());
    }

    #[test]
    fn needs_approval_returns_false_for_safe_equipment() {
        let m = make_manifest("safe", EquipmentType::Skill, vec![], false);
        assert!(!EquipmentRegistry::needs_approval(&m));
    }

    #[test]
    fn needs_approval_returns_true_for_shell_allowed() {
        let m = make_manifest(
            "dangerous",
            EquipmentType::ToolExtension,
            vec!["shell"],
            true,
        );
        assert!(EquipmentRegistry::needs_approval(&m));
    }

    #[test]
    fn needs_approval_returns_true_for_network_allowed() {
        let m = Manifest {
            name: "net-ext".into(),
            version: "1.0.0".into(),
            description: "".into(),
            equipment_type: EquipmentType::ToolExtension,
            tools: vec!["webfetch".into()],
            profiles: vec![],
            mcp_servers: vec![],
            permissions: EquipmentPermissions {
                network: NetworkPermission {
                    allowed: true,
                    allowed_hosts: vec![],
                },
                ..Default::default()
            },
            entry: EquipmentEntry::None {},
        };
        assert!(EquipmentRegistry::needs_approval(&m));
    }

    #[test]
    fn needs_approval_returns_true_for_filesystem_allowed() {
        let m = Manifest {
            name: "fs-ext".into(),
            version: "1.0.0".into(),
            description: "".into(),
            equipment_type: EquipmentType::ToolExtension,
            tools: vec!["read".into()],
            profiles: vec![],
            mcp_servers: vec![],
            permissions: EquipmentPermissions {
                filesystem: FilesystemPermission {
                    allowed: true,
                    allowed_paths: vec![],
                    write_allowed: false,
                },
                ..Default::default()
            },
            entry: EquipmentEntry::None {},
        };
        assert!(EquipmentRegistry::needs_approval(&m));
    }

    #[test]
    fn needs_approval_returns_true_for_browser_allowed() {
        let m = Manifest {
            name: "browser-ext".into(),
            version: "1.0.0".into(),
            description: "".into(),
            equipment_type: EquipmentType::ToolExtension,
            tools: vec!["browser".into()],
            profiles: vec![],
            mcp_servers: vec![],
            permissions: EquipmentPermissions {
                browser: BrowserPermission { allowed: true },
                ..Default::default()
            },
            entry: EquipmentEntry::None {},
        };
        assert!(EquipmentRegistry::needs_approval(&m));
    }

    #[test]
    fn needs_approval_returns_true_for_mcp_server() {
        let mut m = make_manifest("mcp-ext", EquipmentType::ToolExtension, vec![], false);
        m.mcp_servers.push(McpServerConfig {
            id: "repo-tools".into(),
            command: "node".into(),
            args: vec!["server.js".into()],
            env: Default::default(),
            working_dir: None,
            timeout_ms: 1000,
        });

        assert!(EquipmentRegistry::needs_approval(&m));
    }

    #[test]
    fn validate_manifest_rejects_invalid_mcp_server() {
        let mut m = make_manifest("mcp-ext", EquipmentType::ToolExtension, vec![], false);
        m.mcp_servers.push(McpServerConfig {
            id: "repo-tools".into(),
            command: "".into(),
            args: Vec::new(),
            env: Default::default(),
            working_dir: None,
            timeout_ms: 1000,
        });

        let errors = EquipmentRegistry::validate_manifest(&m).expect_err("invalid mcp");

        assert!(errors.iter().any(|error| error.contains("command")));
    }

    #[test]
    fn set_status_disabled_to_active_requires_approval_for_dangerous() {
        let mut reg = EquipmentRegistry::new(tempfile::tempdir().unwrap().path().to_path_buf());
        let m = make_manifest(
            "dangerous",
            EquipmentType::ToolExtension,
            vec!["shell"],
            true,
        );
        let id = m.id();
        reg.loaded.insert(
            id.clone(),
            Equipment {
                id: id.clone(),
                manifest: m,
                dir: PathBuf::from("/tmp/fake"),
                status: EquipmentStatus::Disabled,
            },
        );

        // Without approval callback, dangerous equipment should be denied.
        let result = reg.set_status(&id, EquipmentStatus::Active);
        assert!(result.is_err());

        // With approval callback that returns false, still denied.
        reg.set_approval_callback(Box::new(|_| false));
        let result = reg.set_status(&id, EquipmentStatus::Active);
        assert!(result.is_err());

        // With approval callback that returns true, allowed.
        reg.set_approval_callback(Box::new(|_| true));
        let result = reg.set_status(&id, EquipmentStatus::Active);
        assert!(result.is_ok());
        assert_eq!(reg.get(&id).unwrap().status, EquipmentStatus::Active);
    }

    #[test]
    fn safe_equipment_does_not_need_approval_for_enable() {
        let mut reg = EquipmentRegistry::new(tempfile::tempdir().unwrap().path().to_path_buf());
        let m = make_manifest("safe", EquipmentType::Skill, vec![], false);
        let id = m.id();
        reg.loaded.insert(
            id.clone(),
            Equipment {
                id: id.clone(),
                manifest: m,
                dir: PathBuf::from("/tmp/fake"),
                status: EquipmentStatus::Disabled,
            },
        );

        // Safe equipment should not require approval callback.
        let result = reg.set_status(&id, EquipmentStatus::Active);
        assert!(result.is_ok());
    }

    #[test]
    fn install_dangerous_equipment_requires_approval() {
        let base = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let m = make_manifest(
            "hacker-tool",
            EquipmentType::ToolExtension,
            vec!["shell"],
            true,
        );
        write_equipment(src.path(), "hacker-tool", &m);

        let mut reg = EquipmentRegistry::new(base.path().to_path_buf());

        // Without approval callback, dangerous install should be rejected.
        let result = reg.install_from(&src.path().join("hacker-tool"));
        assert!(result.is_err());

        // With approval callback that returns true, allowed.
        reg.set_approval_callback(Box::new(|_| true));
        let result = reg.install_from(&src.path().join("hacker-tool"));
        assert!(result.is_ok());
    }

    #[test]
    fn equipment_tools_returns_empty_for_non_extension() {
        let reg = EquipmentRegistry::new(tempfile::tempdir().unwrap().path().to_path_buf());
        let tool_reg = ToolRegistry::new();
        let tools = reg.equipment_tools(&tool_reg);
        assert!(tools.is_empty());
    }

    #[test]
    fn equipment_tools_wraps_with_permission_guard() {
        let mut reg = EquipmentRegistry::new(tempfile::tempdir().unwrap().path().to_path_buf());

        // Register a "shell" tool in the equipment registry.
        // Create a minimal shell tool for testing.
        let m = make_manifest("ext", EquipmentType::ToolExtension, vec!["shell"], true);
        let id = m.id();
        reg.loaded.insert(
            id.clone(),
            Equipment {
                id: id.clone(),
                manifest: m,
                dir: PathBuf::from("/tmp/fake"),
                status: EquipmentStatus::Active,
            },
        );

        // Build a ToolRegistry with a "shell" tool.
        use crate::tool::ToolOutput;
        use async_trait::async_trait;
        struct FakeShell;
        #[async_trait]
        impl crate::tool::Tool for FakeShell {
            fn name(&self) -> &str {
                "shell"
            }
            fn description(&self) -> &str {
                "fake"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            async fn execute(&self, _args: &serde_json::Value) -> ToolOutput {
                ToolOutput::success(serde_json::json!({"ok": true}))
            }
        }

        let mut tool_reg = ToolRegistry::new();
        tool_reg.register_ref(Arc::new(FakeShell));

        let tools = reg.equipment_tools(&tool_reg);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].0, "ext");
        assert_eq!(tools[0].1.name(), "shell");
    }

    #[test]
    fn register_tools_adds_to_registry() {
        let mut reg = EquipmentRegistry::new(tempfile::tempdir().unwrap().path().to_path_buf());
        let m = make_manifest("ext", EquipmentType::ToolExtension, vec!["shell"], false);
        let id = m.id();
        reg.loaded.insert(
            id.clone(),
            Equipment {
                id: id.clone(),
                manifest: m,
                dir: PathBuf::from("/tmp/fake"),
                status: EquipmentStatus::Active,
            },
        );

        use crate::tool::ToolOutput;
        use async_trait::async_trait;
        struct FakeShell;
        #[async_trait]
        impl crate::tool::Tool for FakeShell {
            fn name(&self) -> &str {
                "shell"
            }
            fn description(&self) -> &str {
                "fake"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            async fn execute(&self, _args: &serde_json::Value) -> ToolOutput {
                ToolOutput::success(serde_json::json!({"ok": true}))
            }
        }

        let mut tool_reg = ToolRegistry::new();
        tool_reg.register_ref(Arc::new(FakeShell));

        reg.register_tools(&mut tool_reg);
        // shell should now be PermissionGuard-wrapped
        assert!(tool_reg.get("shell").is_some());
    }

    #[test]
    fn unregister_tools_removes_from_registry() {
        let mut reg = EquipmentRegistry::new(tempfile::tempdir().unwrap().path().to_path_buf());
        let m = make_manifest("ext", EquipmentType::ToolExtension, vec!["shell"], false);
        let id = m.id();
        reg.loaded.insert(
            id.clone(),
            Equipment {
                id: id.clone(),
                manifest: m,
                dir: PathBuf::from("/tmp/fake"),
                status: EquipmentStatus::Active,
            },
        );

        use crate::tool::ToolOutput;
        use async_trait::async_trait;
        struct FakeShell;
        #[async_trait]
        impl crate::tool::Tool for FakeShell {
            fn name(&self) -> &str {
                "shell"
            }
            fn description(&self) -> &str {
                "fake"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            async fn execute(&self, _args: &serde_json::Value) -> ToolOutput {
                ToolOutput::success(serde_json::json!({"ok": true}))
            }
        }

        let mut tool_reg = ToolRegistry::new();
        tool_reg.register_ref(Arc::new(FakeShell));
        reg.register_tools(&mut tool_reg);
        assert!(tool_reg.get("shell").is_some());

        let count = reg.unregister_tools(&mut tool_reg, "ext");
        assert_eq!(count, 1);
        assert!(tool_reg.get("shell").is_none());
    }

    #[test]
    fn tool_registry_unregister_returns_false_for_missing() {
        let mut reg = ToolRegistry::new();
        assert!(!reg.unregister("nonexistent"));
    }
}
