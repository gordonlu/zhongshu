use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::tool::Tool;

use super::manifest::{EquipmentId, EquipmentStatus, Manifest};

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
}

impl EquipmentRegistry {
    pub fn new(base_dir: PathBuf) -> Self {
        EquipmentRegistry {
            base_dir,
            loaded: HashMap::new(),
        }
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
                        self.loaded.insert(id.clone(), Equipment {
                            id,
                            manifest,
                            dir: path,
                            status: EquipmentStatus::Active,
                        });
                    }
                    Err(e) => {
                        tracing::warn!("equipment: invalid manifest at {}: {e}", manifest_path.display());
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
                    let name = full.file_stem().map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| rel_path.clone());
                    profiles.push((eq.id.clone(), name, full));
                }
            }
        }
        profiles
    }

    /// Collect tools from active equipment, wrapped with PermissionGuard.
    pub fn equipment_tools(&self) -> Vec<(EquipmentId, Arc<dyn Tool>)> {
        let result = Vec::new();
        for eq in self.loaded.values() {
            if eq.status != EquipmentStatus::Active {
                continue;
            }
            // Tool-extension type equipment provides the tool itself.
            // For now, tool extensions reference built-in tools with restrictions.
            // Future: dynamically compiled/loaded tool extensions.
        }
        result
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
        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }

    /// Install equipment from a source directory into the equipment dir.
    /// If equipment with the same name already exists, upgrades only if
    /// the new version is strictly greater (semver comparison).
    pub fn install_from(&mut self, src: &Path) -> Result<EquipmentId, String> {
        let manifest_path = src.join("manifest.json");
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("cannot read manifest: {e}"))?;
        let manifest: Manifest = serde_json::from_str(&text)
            .map_err(|e| format!("invalid manifest: {e}"))?;

        Self::validate_manifest(&manifest).map_err(|errors| errors.join("; "))?;

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
            tracing::info!("equipment: upgrading '{}' v{} → v{}", id, existing.manifest.version, manifest.version);
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

        self.loaded.insert(id.clone(), Equipment {
            id: id.clone(),
            manifest,
            dir: dest,
            status: EquipmentStatus::Active,
        });

        tracing::info!("equipment: installed '{}'", id);
        Ok(id)
    }

    /// Remove installed equipment (disables + deletes directory).
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        let eq = self.loaded.remove(id).ok_or_else(|| format!("equipment '{}' not found", id))?;
        self.set_status(id, EquipmentStatus::Disabled);
        if eq.dir.exists() {
            std::fs::remove_dir_all(&eq.dir)
                .map_err(|e| format!("cannot remove {}: {e}", eq.dir.display()))?;
        }
        tracing::info!("equipment: removed '{}'", id);
        Ok(())
    }

    pub fn set_status(&mut self, id: &str, status: EquipmentStatus) {
        if let Some(eq) = self.loaded.get_mut(id) {
            eq.status = status;
        }
    }

    /// Write built-in equipment to disk if not already installed.
    pub fn install_defaults(&mut self) {
        for (name, _version, manifest_json, prompt_md) in crate::equipment::builtin::all_builtins() {
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
            if !matches!(eq.manifest.equipment_type, crate::equipment::EquipmentType::Skill) {
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
        v.split('.')
            .filter_map(|s| s.parse::<u32>().ok())
            .collect()
    }
    let a = parse(new);
    let b = parse(old);
    for i in 0..a.len().max(b.len()) {
        let av = a.get(i).copied().unwrap_or(0);
        let bv = b.get(i).copied().unwrap_or(0);
        if av > bv { return true; }
        if av < bv { return false; }
    }
    false // equal
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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

        // Create a valid equipment package.
        let eq_dir = src.path().join("test-tool");
        fs::create_dir_all(&eq_dir).unwrap();
        let manifest = Manifest {
            name: "test-tool".into(),
            version: "1.0.0".into(),
            description: "A test".into(),
            equipment_type: crate::equipment::EquipmentType::Skill,
            tools: vec![],
            profiles: vec![],
            permissions: Default::default(),
            entry: crate::equipment::EquipmentEntry::None {},
        };
        let json = serde_json::to_string_pretty(&manifest).unwrap();
        fs::write(eq_dir.join("manifest.json"), &json).unwrap();

        let mut reg = EquipmentRegistry::new(base.path().to_path_buf());
        let id = reg.install_from(&eq_dir).unwrap();
        assert_eq!(id, "test-tool");

        // Scan in a new registry instance.
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
            equipment_type: crate::equipment::EquipmentType::Skill,
            tools: vec![],
            profiles: vec![],
            permissions: Default::default(),
            entry: crate::equipment::EquipmentEntry::None {},
        };
        assert!(EquipmentRegistry::validate_manifest(&m).is_err());
    }
}
