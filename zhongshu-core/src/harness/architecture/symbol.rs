use std::collections::HashMap;
use std::path::PathBuf;

pub type SymbolId = String;

pub struct SymbolIndex {
    /// symbol_id → file path
    pub symbols: HashMap<SymbolId, PathBuf>,
}

impl SymbolIndex {
    pub fn new() -> Self {
        SymbolIndex { symbols: HashMap::new() }
    }

    pub fn insert(&mut self, id: SymbolId, path: PathBuf) {
        self.symbols.insert(id, path);
    }

    pub fn lookup(&self, id: &str) -> Option<&PathBuf> {
        self.symbols.get(id)
    }

    pub fn update_file(&mut self, path: &PathBuf, items: &[String]) {
        // Remove old entries for this file
        self.symbols.retain(|_, p| p != path);
        // Insert new entries
        for item in items {
            let id = format!("{}::{}", path.display(), item);
            self.symbols.insert(id, path.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn insert_and_lookup() {
        let mut idx = SymbolIndex::new();
        idx.insert("fn foo".into(), PathBuf::from("lib.rs"));
        assert!(idx.lookup("fn foo").is_some());
        assert!(idx.lookup("fn bar").is_none());
    }

    #[test]
    fn update_file_replaces_entries() {
        let mut idx = SymbolIndex::new();
        let p = PathBuf::from("lib.rs");
        idx.insert("fn old".into(), p.clone());
        idx.update_file(&p, &["fn new".into()]);
        assert!(idx.lookup("fn old").is_none());
        assert!(idx.lookup("lib.rs::fn new").is_some());
    }
}
