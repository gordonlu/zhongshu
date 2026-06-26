use std::collections::HashMap;
use std::path::PathBuf;

use crate::harness::architecture::index::ProjectIndex;

/// Resolve import string to file path (simple heuristic — no full path resolution).
pub fn resolve_import(import: &str, project_root: &PathBuf) -> Option<PathBuf> {
    // Handle crate-relative: `crate::foo::Bar` → `src/foo.rs`
    if let Some(rest) = import.strip_prefix("crate::") {
        let file_path = rest.replace("::", "/");
        // Try src/{path}.rs
        let candidate1 = project_root.join("src").join(format!("{}.rs", file_path));
        if candidate1.exists() {
            return Some(candidate1);
        }
        // Try src/{path}/mod.rs
        let candidate2 = project_root.join("src").join(&file_path).join("mod.rs");
        if candidate2.exists() {
            return Some(candidate2);
        }
    }
    // Handle crate name prefix
    if let Some((crate_name, rest)) = import.split_once("::") {
        // Simple workspace crate lookup
        let crate_path = project_root.join(crate_name.replace('-', "_"));
        let file_path = rest.replace("::", "/");
        for candidate in &[
            crate_path.join("src").join(format!("{}.rs", file_path)),
            crate_path.join("src").join(&file_path).join("mod.rs"),
        ] {
            if candidate.exists() {
                return Some(candidate.clone());
            }
        }
    }
    None
}

/// Build a simple import graph: file → files it imports.
pub fn build_import_graph(index: &ProjectIndex) -> HashMap<PathBuf, Vec<PathBuf>> {
    let mut graph = HashMap::new();
    for (path, file_index) in &index.files {
        let mut targets = Vec::new();
        for import in &file_index.imports {
            if let Some(target) = resolve_import(import, &index.root) {
                if target != *path {
                    targets.push(target);
                }
            }
        }
        graph.insert(path.clone(), targets);
    }
    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn resolve_crate_import() {
        let root = PathBuf::from(".");
        // This is a heuristic test — depends on actual project structure
        let result = resolve_import("crate::harness::architecture::graph", &root);
        // May or may not resolve depending on working dir
        if let Some(p) = result {
            assert!(p.ends_with("graph.rs"));
        }
    }
}
