use std::path::PathBuf;

/// AST-level change representation.
#[derive(Debug, Clone, PartialEq)]
pub enum AstChange {
    FunctionAdded { symbol: String },
    FunctionRemoved { symbol: String },
    FunctionSignatureChanged { symbol: String },
    FunctionBodyChanged { symbol: String },
    ImportAdded { file: PathBuf, import: String },
    ImportRemoved { file: PathBuf, import: String },
}

use crate::harness::architecture::index::FileIndex;

/// Compute structural diff between old and new file indices.
pub fn compute_diff(old: Option<&FileIndex>, new: &FileIndex) -> Vec<AstChange> {
    let mut changes = Vec::new();
    let old = match old {
        Some(o) => o,
        None => {
            // New file: everything is added
            for item in &new.items {
                changes.push(AstChange::FunctionAdded {
                    symbol: item.clone(),
                });
            }
            for import in &new.imports {
                changes.push(AstChange::ImportAdded {
                    file: new.path.clone(),
                    import: import.clone(),
                });
            }
            return changes;
        }
    };

    // Detect removed items
    for item in &old.items {
        if !new.items.contains(item) {
            changes.push(AstChange::FunctionRemoved {
                symbol: item.clone(),
            });
        }
    }

    // Detect added items
    for item in &new.items {
        if !old.items.contains(item) {
            changes.push(AstChange::FunctionAdded {
                symbol: item.clone(),
            });
        }
    }

    // Detect import changes
    for import in &old.imports {
        if !new.imports.contains(import) {
            changes.push(AstChange::ImportRemoved {
                file: new.path.clone(),
                import: import.clone(),
            });
        }
    }
    for import in &new.imports {
        if !old.imports.contains(import) {
            changes.push(AstChange::ImportAdded {
                file: new.path.clone(),
                import: import.clone(),
            });
        }
    }

    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_index(path: &str, items: Vec<&str>, imports: Vec<&str>) -> FileIndex {
        FileIndex {
            path: PathBuf::from(path),
            items: items.into_iter().map(String::from).collect(),
            imports: imports.into_iter().map(String::from).collect(),
            parse_error: None,
        }
    }

    #[test]
    fn new_file_adds_everything() {
        let new = make_index("test.rs", vec!["fn foo"], vec!["crate::bar"]);
        let changes = compute_diff(None, &new);
        assert_eq!(changes.len(), 2);
    }

    #[test]
    fn removed_function_detected() {
        let old = make_index("test.rs", vec!["fn foo", "fn bar"], vec![]);
        let new = make_index("test.rs", vec!["fn foo"], vec![]);
        let changes = compute_diff(Some(&old), &new);
        assert!(changes.contains(&AstChange::FunctionRemoved {
            symbol: "fn bar".into()
        }));
    }

    #[test]
    fn added_import_detected() {
        let old = make_index("test.rs", vec![], vec!["std::collections"]);
        let new = make_index("test.rs", vec![], vec!["std::collections", "std::sync"]);
        let changes = compute_diff(Some(&old), &new);
        assert!(changes
            .iter()
            .any(|c| matches!(c, AstChange::ImportAdded { import, .. } if import == "std::sync")));
    }
}
