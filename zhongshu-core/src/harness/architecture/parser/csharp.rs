use std::path::PathBuf;

use crate::harness::architecture::index::FileIndex;

pub fn parse_csharp(path: PathBuf, content: &str) -> FileIndex {
    let imports = extract_imports(content);
    let items = extract_items(content);
    FileIndex {
        path,
        imports,
        items,
        parse_error: None,
    }
}

fn extract_imports(content: &str) -> Vec<String> {
    let mut imports = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("using ") {
            if let Some(ns) = rest.strip_suffix(';') {
                let ns = ns.trim();
                if !ns.starts_with("static ") && !ns.starts_with("new ") {
                    imports.push(format!("using {}", ns));
                }
            }
        }
    }
    imports
}

fn extract_items(content: &str) -> Vec<String> {
    let mut items = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();

        // Detect namespace
        if let Some(ns) = line.strip_prefix("namespace ") {
            let name = ns
                .split('{')
                .next()
                .unwrap_or(ns)
                .trim_end_matches(|c| c == '}' || c == ';')
                .trim();
            if !name.is_empty() {
                items.push(format!("namespace {}", name));
            }
            i += 1;
            continue;
        }

        // Detect type declarations with possible modifiers
        // Patterns: public class Foo, private struct Bar, internal interface IBaz, etc.
        let type_patterns = [
            ("class ", "class"),
            ("struct ", "struct"),
            ("interface ", "interface"),
            ("enum ", "enum"),
            ("record ", "record"),
        ];

        let line_no_attrs = strip_attributes(line);
        let mut matched = false;
        for (keyword, label) in &type_patterns {
            if let Some(name) = extract_declaration_name(&line_no_attrs, keyword) {
                items.push(format!("{} {}", label, name));
                matched = true;
                break;
            }
        }
        if matched {
            i += 1;
            continue;
        }

        // Detect methods: visibility? return_type Name(...)
        if let Some((name, is_async)) = extract_method(&line_no_attrs) {
            let prefix = if is_async { "async fn " } else { "fn " };
            items.push(format!("{}{}", prefix, name));
            i += 1;
            continue;
        }

        // Detect properties: public type Name { get; set; }
        if let Some(name) = extract_property(&line_no_attrs) {
            items.push(format!("property {}", name));
        }

        i += 1;
    }

    items
}

fn strip_attributes(line: &str) -> &str {
    if line.trim_start().starts_with('[') && line.trim_end().ends_with(']') {
        // Single-line attribute — skip to next line's content
        // (handled by looking at actual line content)
        line
    } else {
        line
    }
}

fn extract_declaration_name(line: &str, keyword: &str) -> Option<String> {
    let idx = line.find(keyword)?;
    let after = line[idx + keyword.len()..].trim();
    // The name comes before any '<' (generics) or ':' (base type) or '{' (body)
    let name = after
        .split(|c: char| c == '<' || c == ':' || c == '{' || c == '(' || c == ' ' || c == '\n')
        .next()
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn extract_method(line: &str) -> Option<(String, bool)> {
    let line = line.trim();
    let is_async = line.contains("async ");

    // Skip non-method lines
    if line.starts_with("using ")
        || line.starts_with("namespace ")
        || line.starts_with("class ")
        || line.starts_with("struct ")
        || line.starts_with("interface ")
        || line.starts_with("enum ")
        || line.starts_with('[')
        || line.starts_with('}')
        || line.starts_with('{')
    {
        return None;
    }

    // Check for method pattern: optional modifiers + return type + Name(...)
    // Remove modifiers
    let after_mods = strip_modifiers(line);

    // Find '(' which indicates a method
    let paren = after_mods.find('(')?;
    let before_paren = &after_mods[..paren].trim();

    // Extract the last word before '(' — that's the method name
    let name = before_paren.split_whitespace().last()?;

    // Basic validation: exclude keywords and operators
    if name.len() < 2 || name.contains('(') || name.contains(')') {
        return None;
    }
    // Explicit operator, indexer, conversion
    if name.starts_with("operator ") || name == "this" {
        return None;
    }

    Some((name.to_string(), is_async))
}

fn extract_property(line: &str) -> Option<String> {
    let line = line.trim();
    if !line.contains("{ get") && !line.contains("{ set") && !line.contains("{ init") {
        return None;
    }
    let after_mods = strip_modifiers(line);
    let brace = after_mods.find('{')?;
    let before = &after_mods[..brace].trim();
    let name = before.split_whitespace().last()?;
    if name.len() < 2 {
        return None;
    }
    Some(name.to_string())
}

fn strip_modifiers(line: &str) -> &str {
    let mut s = line;
    for modif in &[
        "public ",
        "private ",
        "protected ",
        "internal ",
        "static ",
        "virtual ",
        "override ",
        "abstract ",
        "sealed ",
        "async ",
        "unsafe ",
        "extern ",
        "readonly ",
        "volatile ",
        "new ",
        "partial ",
        "record ",
    ] {
        if s.starts_with(modif) {
            s = &s[modif.len()..];
            return strip_modifiers(s);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_csharp_usings() {
        let code = r#"
using System;
using System.Collections.Generic;
using System.Threading.Tasks;
"#;
        let index = parse_csharp("test.cs".into(), code);
        assert!(index.imports.contains(&"using System".to_string()));
        assert!(index
            .imports
            .contains(&"using System.Collections.Generic".to_string()));
    }

    #[test]
    fn parse_csharp_types() {
        let code = r#"
public class UserService { }
private struct Result { }
internal interface IRepository { }
public enum Status { Active, Inactive }
public record Person(string Name);
"#;
        let index = parse_csharp("models.cs".into(), code);
        assert!(index.items.iter().any(|s| s == "class UserService"));
        assert!(index.items.iter().any(|s| s == "struct Result"));
        assert!(index.items.iter().any(|s| s == "interface IRepository"));
        assert!(index.items.iter().any(|s| s == "enum Status"));
        assert!(index.items.iter().any(|s| s == "record Person"));
    }

    #[test]
    fn parse_csharp_methods() {
        let code = r#"
public string GetName(int id) { return ""; }
public async Task<User> FindUserAsync(string email) { return null; }
private void Validate() { }
"#;
        let index = parse_csharp("service.cs".into(), code);
        assert!(index.items.iter().any(|s| s == "fn GetName"));
        assert!(index.items.iter().any(|s| s == "async fn FindUserAsync"));
        assert!(index.items.iter().any(|s| s == "fn Validate"));
    }

    #[test]
    fn parse_csharp_properties() {
        let code = r#"
public string Name { get; set; }
public int Age { get; private set; }
"#;
        let index = parse_csharp("model.cs".into(), code);
        assert!(index.items.iter().any(|s| s == "property Name"));
        assert!(index.items.iter().any(|s| s == "property Age"));
    }

    #[test]
    fn parse_csharp_namespace() {
        let code = "namespace MyApp.Services { }";
        let index = parse_csharp("test.cs".into(), code);
        assert!(index.items.iter().any(|s| s == "namespace MyApp.Services"));
    }

    #[test]
    fn parse_csharp_attributes_do_not_confuse() {
        let code = r#"
[HttpGet("{id}")]
public async Task<IActionResult> GetUser(int id) { return Ok(); }
"#;
        let index = parse_csharp("controller.cs".into(), code);
        assert!(index.items.iter().any(|s| s == "async fn GetUser"));
    }

    #[test]
    fn parse_csharp_generic_types() {
        let code = r#"
public class Repository<T> where T : class { }
public interface ICache<TKey, TValue> { }
"#;
        let index = parse_csharp("generics.cs".into(), code);
        assert!(index.items.iter().any(|s| s == "class Repository"));
        assert!(index.items.iter().any(|s| s == "interface ICache"));
    }
}
