use std::collections::HashSet;

use syn::{File, Item, Visibility};

/// Snapshot of a file's public API surface.
pub struct ApiSnapshot;

impl ApiSnapshot {
    /// Compare public API before vs after, returning breaking change descriptions.
    pub fn check_compatibility(before: &str, after: &str) -> Vec<String> {
        let before_pub = extract_pub_items(before);
        let after_pub = extract_pub_items(after);

        let mut breaks = Vec::new();
        for item in &before_pub {
            if !after_pub.contains(item) {
                breaks.push(format!("公共 API 被移除: {item}"));
            }
        }
        breaks
    }
}

fn extract_pub_items(content: &str) -> HashSet<String> {
    let ast: File = match syn::parse_file(content) {
        Ok(f) => f,
        Err(_) => return HashSet::new(),
    };

    let mut items = HashSet::new();
    for item in &ast.items {
        let name = match item {
            Item::Fn(f) if is_pub(&f.vis) => Some(format!("fn {}", f.sig.ident)),
            Item::Struct(s) if is_pub(&s.vis) => Some(format!("struct {}", s.ident)),
            Item::Enum(e) if is_pub(&e.vis) => Some(format!("enum {}", e.ident)),
            Item::Trait(t) if is_pub(&t.vis) => Some(format!("trait {}", t.ident)),
            Item::Type(t) if is_pub(&t.vis) => Some(format!("type {}", t.ident)),
            Item::Const(c) if is_pub(&c.vis) => Some(format!("const {}", c.ident)),
            Item::Mod(m) if is_pub(&m.vis) => Some(format!("mod {}", m.ident)),
            _ => None,
        };
        if let Some(name) = name {
            items.insert(name);
        }
    }
    items
}

fn is_pub(vis: &Visibility) -> bool {
    matches!(vis, Visibility::Public(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_removed_pub_fn() {
        let before = "pub fn foo() {}";
        let after = "fn foo() {}";
        let breaks = ApiSnapshot::check_compatibility(before, after);
        assert!(breaks.iter().any(|b| b.contains("foo")));
    }

    #[test]
    fn no_break_for_private_fn() {
        let before = "fn foo() {}";
        let after = "";
        let breaks = ApiSnapshot::check_compatibility(before, after);
        assert!(breaks.is_empty());
    }

    #[test]
    fn detects_removed_pub_struct() {
        let before = "pub struct Foo;";
        let after = "struct Foo;";
        let breaks = ApiSnapshot::check_compatibility(before, after);
        assert!(breaks.iter().any(|b| b.contains("Foo")));
    }

    #[test]
    fn unchanged_api_returns_empty() {
        let content = "pub fn foo() {}\npub struct Bar {}";
        let breaks = ApiSnapshot::check_compatibility(content, content);
        assert!(breaks.is_empty());
    }
}
