use crate::harness::architecture::diff::AstChange;
use crate::harness::architecture::graph;
use crate::harness::architecture::index::ProjectIndex;

/// Analyze a set of changes and report impacted symbols/files.
pub fn analyze(changes: &[AstChange], index: &ProjectIndex) -> Vec<String> {
    let mut results = Vec::new();

    for change in changes {
        match change {
            AstChange::FunctionRemoved { symbol } => {
                let usages = find_referrers(symbol, index);
                if !usages.is_empty() {
                    results.push(format!(
                        "函数 {symbol} 被移除，影响 {} 处引用",
                        usages.len()
                    ));
                }
            }
            AstChange::FunctionAdded { symbol } => {
                results.push(format!("添加了新函数 {symbol}"));
            }
            AstChange::FunctionSignatureChanged { symbol } => {
                let usages = find_referrers(symbol, index);
                if !usages.is_empty() {
                    results.push(format!(
                        "函数 {symbol} 签名变更，影响 {} 处调用",
                        usages.len()
                    ));
                }
            }
            AstChange::FunctionBodyChanged { symbol } => {
                results.push(format!("函数 {symbol} 实现变更"));
            }
            AstChange::ImportAdded { file, import } => {
                if let Some(target) = graph::resolve_import(import, &index.root) {
                    results.push(format!(
                        "{} 新增导入 {import} → {}",
                        file.display(),
                        target.display()
                    ));
                } else {
                    results.push(format!("{} 新增导入 {import}", file.display()));
                }
            }
            AstChange::ImportRemoved { file, import } => {
                results.push(format!("{} 移除了导入 {import}", file.display()));
            }
        }
    }

    results
}

/// Find files that import something matching the symbol name (substring match).
fn find_referrers(symbol: &str, index: &ProjectIndex) -> Vec<std::path::PathBuf> {
    let mut referrers: Vec<std::path::PathBuf> = index
        .files
        .iter()
        .filter(|(_path, fi)| {
            let is_definer = fi.items.iter().any(|i| i == symbol);
            if is_definer {
                return false;
            }
            let sym_lower = symbol.to_lowercase();
            let short = symbol
                .strip_prefix("fn ")
                .or_else(|| symbol.strip_prefix("struct "))
                .or_else(|| symbol.strip_prefix("enum "))
                .or_else(|| symbol.strip_prefix("trait "))
                .unwrap_or(symbol);
            fi.imports
                .iter()
                .any(|i| i.to_lowercase().contains(&sym_lower))
                || fi.imports.iter().any(|i| {
                    let i_lower = i.to_lowercase();
                    i_lower.contains(&short.to_lowercase())
                        || short.to_lowercase().contains(&i_lower)
                })
        })
        .map(|(path, _)| path.clone())
        .collect();

    referrers.sort();
    referrers.dedup();
    referrers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::architecture::diff::AstChange;
    use std::path::PathBuf;

    #[test]
    fn added_function_reported() {
        let changes = vec![AstChange::FunctionAdded {
            symbol: "fn foo".into(),
        }];
        let index = ProjectIndex::new(PathBuf::from("."));
        let results = analyze(&changes, &index);
        assert!(results.iter().any(|r| r.contains("foo")));
    }

    #[test]
    fn removed_import_reported() {
        let changes = vec![AstChange::ImportRemoved {
            file: PathBuf::from("test.rs"),
            import: "std::collections".into(),
        }];
        let index = ProjectIndex::new(PathBuf::from("."));
        let results = analyze(&changes, &index);
        assert!(results.iter().any(|r| r.contains("test.rs")));
    }
}
