use serde::{Deserialize, Serialize};

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
}

impl Default for EquipmentPermissions {
    fn default() -> Self {
        EquipmentPermissions {
            shell: ShellPermission::default(),
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
            permissions: EquipmentPermissions {
                shell: ShellPermission {
                    allowed: false,
                    allowed_commands: vec!["cargo".into(), "git".into()],
                },
            },
            entry: EquipmentEntry::Workflow("workflow.yaml".into()),
        };
        let json = serde_json::to_string_pretty(&m).unwrap();
        let parsed: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "rust-release");
        assert_eq!(parsed.equipment_type, EquipmentType::Workflow);
        assert_eq!(parsed.permissions.shell.allowed_commands, vec!["cargo", "git"]);
    }

    #[test]
    fn manifest_defaults() {
        let json = r#"{"name":"test","version":"0.1.0"}"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.equipment_type, EquipmentType::Skill);
        assert!(m.tools.is_empty());
        assert!(!m.permissions.shell.allowed);
    }
}
