use std::path::Path;

pub fn workspace_snapshot(root: &Path) -> std::collections::HashMap<String, String> {
    let mut snapshot = std::collections::HashMap::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "rs").unwrap_or(false) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    use std::hash::{Hash, Hasher};
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    content.hash(&mut hasher);
                    let hash = format!("{:x}", hasher.finish());
                    snapshot.insert(path.to_string_lossy().to_string(), hash);
                }
            }
        }
    }
    snapshot
}
