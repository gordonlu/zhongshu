use std::path::PathBuf;
use syn::{File, Item, UseTree};

use crate::harness::architecture::index::FileIndex;

pub fn parse_rust(path: PathBuf, content: &str) -> FileIndex {
    let ast: File = match syn::parse_file(content) {
        Ok(f) => f,
        Err(_) => {
            return FileIndex {
                path,
                imports: Vec::new(),
                items: Vec::new(),
                parse_error: Some("syn parse failed".into()),
            };
        }
    };

    let imports = extract_imports(&ast);
    let items = extract_items(&ast);

    FileIndex {
        path,
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
            if rest.is_empty() {
                segment
            } else {
                format!("{}::{}", segment, rest)
            }
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
    path.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

/// Expand a use tree into individual leaf import paths.
/// Handles grouped imports (`a::{b, c}`), aliases (`a as b` → `a`),
/// and globs (`a::*`).
pub fn expanded_rust_imports(content: &str) -> Vec<String> {
    let ast: syn::File = match syn::parse_file(content) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let mut result = Vec::new();
    for item in &ast.items {
        if let syn::Item::Use(u) = item {
            collect_expanded(&u.tree, String::new(), &mut result);
        }
    }
    result
}

fn collect_expanded(tree: &UseTree, prefix: String, result: &mut Vec<String>) {
    match tree {
        UseTree::Path(p) => {
            let seg = p.ident.to_string();
            let new_prefix = if prefix.is_empty() {
                seg
            } else {
                format!("{prefix}::{seg}")
            };
            collect_expanded(&p.tree, new_prefix, result);
        }
        UseTree::Name(n) => {
            let name = n.ident.to_string();
            result.push(if prefix.is_empty() {
                name
            } else {
                format!("{prefix}::{name}")
            });
        }
        UseTree::Rename(r) => {
            let name = r.ident.to_string();
            result.push(if prefix.is_empty() {
                name
            } else {
                format!("{prefix}::{name}")
            });
        }
        UseTree::Glob(_) => {
            result.push(format!("{prefix}::*"));
        }
        UseTree::Group(g) => {
            for item in &g.items {
                collect_expanded(item, prefix.clone(), result);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_rust_functions() {
        let content = r#"
use std::collections::HashMap;
pub fn foo() -> i32 { 42 }
struct Bar { x: i32 }
"#;
        let index = parse_rust(PathBuf::from("test.rs"), content);
        assert!(index
            .imports
            .contains(&"std::collections::HashMap".to_string()));
        assert!(index.items.contains(&"fn foo".to_string()));
        assert!(index.items.contains(&"struct Bar".to_string()));
        assert!(index.parse_error.is_none());
    }
}
