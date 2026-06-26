use std::path::PathBuf;

use crate::harness::architecture::index::FileIndex;

pub fn parse_java(path: PathBuf, content: &str) -> FileIndex {
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
        if let Some(rest) = trimmed.strip_prefix("import ") {
            let name = rest.strip_suffix(';').unwrap_or(rest).trim();
            if !name.is_empty() && !name.contains('*') {
                imports.push(format!("import {}", name));
            }
        }
    }
    imports
}

fn extract_items(content: &str) -> Vec<String> {
    let mut items = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();

        // package
        if let Some(rest) = trimmed.strip_prefix("package ") {
            let name = rest.strip_suffix(';').unwrap_or(rest).trim();
            if !name.is_empty() {
                items.push(format!("package {}", name));
            }
            continue;
        }

        // Class / interface / enum / @interface
        let type_keywords = [
            ("class ", "class"),
            ("interface ", "interface"),
            ("enum ", "enum"),
            ("@interface ", "annotation"),
            ("record ", "record"),
        ];
        let mut matched = false;
        for (keyword, label) in &type_keywords {
            if let Some(name) = extract_name(trimmed, keyword) {
                items.push(format!("{} {}", label, name));
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }

        // Method: visibility? return_type Name(...)
        if !trimmed.starts_with("import ")
            && !trimmed.starts_with("package ")
            && !trimmed.starts_with('{')
            && !trimmed.starts_with('}')
            && trimmed.contains('(')
            && trimmed.contains(')')
            && !trimmed.starts_with("//")
        {
            if let Some(name) = extract_method_name(trimmed) {
                let is_async = trimmed.contains("CompletableFuture") || trimmed.contains("async");
                let prefix = if is_async { "async fn " } else { "fn " };
                items.push(format!("{}{}", prefix, name));
            }
        }
    }
    items
}

fn extract_name(line: &str, keyword: &str) -> Option<String> {
    let idx = line.find(keyword)?;
    let after = line[idx + keyword.len()..].trim();
    let name = after
        .split(|c: char| c == ' ' || c == '{' || c == '(' || c == '<' || c == '\n')
        .next()
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn extract_method_name(line: &str) -> Option<String> {
    let line = strip_modifiers(line);
    // Remove return type (the word before '(' that isn't a keyword)
    let paren = line.find('(')?;
    let before_paren = &line[..paren].trim();
    let name = before_paren.split_whitespace().last()?;
    if name.len() < 2 || name == "class" || name == "interface" || name == "enum" {
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
        "static ",
        "final ",
        "abstract ",
        "synchronized ",
        "transient ",
        "volatile ",
        "native ",
        "strictfp ",
        "default ",
        "sealed ",
        "non-sealed ",
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
    fn parse_java_imports() {
        let code = r#"
import java.util.List;
import java.util.ArrayList;
import org.springframework.web.bind.annotation.*;
"#;
        let index = parse_java("Main.java".into(), code);
        assert!(index.imports.contains(&"import java.util.List".to_string()));
        assert!(index
            .imports
            .contains(&"import java.util.ArrayList".to_string()));
        // Wildcard imports should be excluded per current design
        assert!(!index.imports.iter().any(|i| i.contains('*')));
    }

    #[test]
    fn parse_java_types() {
        let code = r#"
public class UserService { }
private interface Repository { }
public enum Status { ACTIVE, INACTIVE }
"#;
        let index = parse_java("Models.java".into(), code);
        assert!(index.items.iter().any(|s| s == "class UserService"));
        assert!(index.items.iter().any(|s| s == "interface Repository"));
        assert!(index.items.iter().any(|s| s == "enum Status"));
    }

    #[test]
    fn parse_java_methods() {
        let code = r#"
public String getName(int id) { return ""; }
private CompletableFuture<User> findUserAsync(String email) { return null; }
"#;
        let index = parse_java("Service.java".into(), code);
        assert!(index.items.iter().any(|s| s == "fn getName"));
        assert!(index.items.iter().any(|s| s == "async fn findUserAsync"));
    }
}
