use std::path::PathBuf;
use syn::{File, Item, UseTree};

use crate::harness::architecture::index::FileIndex;

/// Parse a Rust source file and return a FileIndex.
pub fn parse_file(path: &PathBuf, content: &str) -> FileIndex {
    let ast: File = match syn::parse_file(content) {
        Ok(f) => f,
        Err(_) => {
            return FileIndex {
                path: path.clone(),
                imports: Vec::new(),
                items: Vec::new(),
                parse_error: Some("syn parse failed".into()),
            };
        }
    };

    let imports = extract_imports(&ast);
    let items = extract_items(&ast);

    FileIndex {
        path: path.clone(),
        imports,
        items,
        parse_error: None,
    }
}

fn extract_imports(ast: &File) -> Vec<String> {
    let mut imports = Vec::new();
    for item in &ast.items {
        if let Item::Use(u) = item {
            imports.push(format_use_tree(&u.tree));
        }
    }
    imports
}

fn format_use_tree(tree: &UseTree) -> String {
    match tree {
        UseTree::Path(p) => {
            let segment = p.ident.to_string();
            let rest = format_use_tree(&p.tree);
            if rest.is_empty() { segment } else { format!("{}::{}", segment, rest) }
        }
        UseTree::Name(n) => n.ident.to_string(),
        UseTree::Rename(r) => format!("{} as {}", r.ident, r.rename),
        UseTree::Glob(_) => "*".into(),
        UseTree::Group(g) => {
            let items: Vec<String> = g.items.iter().map(format_use_tree).collect();
            format!("{{{}}}", items.join(", "))
        }
    }
}

fn extract_items(ast: &File) -> Vec<String> {
    let mut names = Vec::new();
    for item in &ast.items {
        match item {
            Item::Fn(f) => names.push(format!("fn {}", f.sig.ident)),
            Item::Struct(s) => names.push(format!("struct {}", s.ident)),
            Item::Enum(e) => names.push(format!("enum {}", e.ident)),
            Item::Trait(t) => names.push(format!("trait {}", t.ident)),
            Item::Impl(i) => {
                if let Some((_, path, _)) = &i.trait_ {
                    names.push(format!("impl {} for ...", path_to_string(path)));
                } else {
                    if let syn::Type::Path(tp) = &*i.self_ty {
                        if let Some(seg) = tp.path.segments.last() {
                            names.push(format!("impl {}", seg.ident));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    names
}

fn path_to_string(path: &syn::Path) -> String {
    path.segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_simple_file() {
        let content = r#"
use std::collections::HashMap;
pub fn foo() -> i32 { 42 }
struct Bar { x: i32 }
"#;
        let index = parse_file(&PathBuf::from("test.rs"), content);
        assert!(index.imports.contains(&"std::collections::HashMap".to_string()));
        assert!(index.items.contains(&"fn foo".to_string()));
        assert!(index.items.contains(&"struct Bar".to_string()));
        assert!(index.parse_error.is_none());
    }

    #[test]
    fn parse_malformed_file_does_not_panic() {
        let content = "fn foo( {";
        let index = parse_file(&PathBuf::from("bad.rs"), content);
        assert!(index.parse_error.is_some());
    }
}
