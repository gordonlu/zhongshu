use std::collections::HashMap;
use std::path::PathBuf;

use crate::harness::architecture::symbol::SymbolIndex;

/// Index of a single source file.
#[derive(Debug, Clone)]
pub struct FileIndex {
    pub path: PathBuf,
    pub imports: Vec<String>,
    pub items: Vec<String>,
    pub parse_error: Option<String>,
}

/// The project-wide index, built from workspace scan and updated incrementally.
#[derive(Debug, Clone)]
pub struct ProjectIndex {
    pub root: PathBuf,
    pub files: HashMap<PathBuf, FileIndex>,
    pub symbols: SymbolIndex,
}

impl ProjectIndex {
    pub fn new(root: PathBuf) -> Self {
        ProjectIndex {
            root,
            files: HashMap::new(),
            symbols: SymbolIndex::new(),
        }
    }

    /// Insert or update a single file's index.
    pub fn update_file(&mut self, path: PathBuf, content: &str) {
        let index = super::rust_ast::parse_file(&path, content);
        let items = index.items.clone();
        self.symbols.update_file(&path, &items);
        self.files.insert(path, index);
    }

    /// Scan a directory recursively for .rs files.
    pub fn scan_dir(&mut self, dir: &PathBuf) {
        if !dir.exists() {
            return;
        }
        for entry in walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path().to_path_buf();
            if path.extension().map(|e| e == "rs").unwrap_or(false) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    self.update_file(path, &content);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn scan_and_query() {
        let dir = PathBuf::from("src/harness/architecture");
        let mut idx = ProjectIndex::new(PathBuf::from("."));
        // Just verify it doesn't crash and finds some files
        idx.scan_dir(&dir);
        // At minimum it should have found its own files
        assert!(
            !idx.files.is_empty(),
            "should have found architecture files"
        );
    }

    #[test]
    fn update_file_replaces() {
        let mut idx = ProjectIndex::new(PathBuf::from("."));
        let path = PathBuf::from("test.rs");
        idx.update_file(path.clone(), "pub fn old() {}");
        assert!(idx.files[&path].items.contains(&"fn old".to_string()));

        idx.update_file(path.clone(), "pub fn new() {}");
        assert!(idx.files[&path].items.contains(&"fn new".to_string()));
        assert!(!idx.files[&path].items.contains(&"fn old".to_string()));
    }
}
