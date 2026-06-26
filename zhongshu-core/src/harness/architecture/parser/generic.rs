use std::path::PathBuf;

use crate::harness::architecture::index::FileIndex;

/// Generic parser for JS, TS, JSX, CSS, HTML, JSON, TOML, YAML, Markdown, SQL, Shell.
/// Uses simple regex/line-based extraction. Accuracy is lower than language-specific
/// parsers but sufficient for dependency scanning and architecture rule evaluation.
pub fn parse_generic(path: PathBuf, content: &str) -> FileIndex {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let (imports, items) = match ext.as_str() {
        "js" | "mjs" | "cjs" => parse_javascript(content),
        "ts" | "tsx" => parse_typescript(content),
        "jsx" => parse_jsx(content),
        "css" | "scss" | "less" | "sass" => parse_css(content),
        "html" | "htm" => parse_html(content),
        "json" => (vec!["import json".into()], vec![]),
        "toml" => (vec![], vec![]),
        "yaml" | "yml" => (vec![], vec![]),
        "md" => (vec![], vec![]),
        "xml" | "svg" => (vec![], vec![]),
        "sql" => (vec![], extract_sql_symbols(content)),
        "sh" | "bash" | "zsh" => (vec![], extract_shell_functions(content)),
        _ => (vec![], vec![]),
    };
    FileIndex {
        path,
        imports,
        items,
        parse_error: None,
    }
}

// ── JavaScript ────────────────────────────────────────────────────────

fn parse_javascript(content: &str) -> (Vec<String>, Vec<String>) {
    let imports = extract_js_imports(content);
    let items = extract_js_items(content);
    (imports, items)
}

fn parse_typescript(content: &str) -> (Vec<String>, Vec<String>) {
    let mut imports = extract_js_imports(content);
    let mut items = extract_js_items(content);

    // TypeScript-specific: interface, type, enum
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("interface ") {
            let name = rest.split_whitespace().next().unwrap_or("");
            if !name.is_empty() {
                items.push(format!("interface {}", name));
            }
        }
        if let Some(rest) = trimmed.strip_prefix("type ") {
            let name = rest.split_whitespace().next().unwrap_or("");
            if !name.is_empty() && !name.starts_with('{') {
                items.push(format!("type {}", name));
            }
        }
        if let Some(rest) = trimmed.strip_prefix("enum ") {
            let name = rest.split_whitespace().next().unwrap_or("");
            if !name.is_empty() {
                items.push(format!("enum {}", name));
            }
        }
    }

    (imports, items)
}

fn parse_jsx(content: &str) -> (Vec<String>, Vec<String>) {
    let (imports, mut items) = parse_typescript(content);
    // Extract React component names (functions returning JSX)
    for line in content.lines() {
        let trimmed = line.trim();
        // Component: export default function Foo() or function Foo()
        if let Some(rest) = trimmed.strip_prefix("function ") {
            let name = rest.split('(').next().unwrap_or("").trim();
            if !name.is_empty()
                && name
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
            {
                items.push(format!("component {}", name));
            }
        }
        // Arrow component: const Foo = () =>
        if trimmed.starts_with("const ") && trimmed.contains("=>") {
            let name = trimmed.split_whitespace().nth(1).unwrap_or("");
            if !name.is_empty()
                && name
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
            {
                items.push(format!("component {}", name));
            }
        }
    }
    (imports, items)
}

/// Trim quotes and trailing semicolon from a module name.
fn clean_module(s: &str) -> String {
    s.trim()
        .trim_end_matches(';')
        .trim_matches('\'')
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

fn extract_js_imports(content: &str) -> Vec<String> {
    let mut imports = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        // import X from 'y'
        if let Some(rest) = trimmed.strip_prefix("import ") {
            if let Some(source) = rest.strip_prefix('{') {
                // import { a, b } from 'x'
                if let Some(from) = source.split("} from ").nth(1) {
                    let module = clean_module(from);
                    if !module.is_empty() {
                        imports.push(format!("import {}", module));
                    }
                }
            } else {
                let parts: Vec<&str> = rest.split("from ").collect();
                if parts.len() == 2 {
                    let module = clean_module(parts[1]);
                    if !module.is_empty() {
                        imports.push(format!("import {}", module));
                    }
                } else if let Some(source) = rest.split_whitespace().next() {
                    // import 'module'
                    let module = clean_module(source);
                    if !module.is_empty() {
                        imports.push(format!("import {}", module));
                    }
                }
            }
        }
        // require('x')
        if let Some(start) = trimmed.find("require(") {
            let after = &trimmed[start + 8..];
            if let Some(end) = after.find(')') {
                let module = clean_module(&after[..end]);
                if !module.is_empty() {
                    imports.push(format!("import {}", module));
                }
            }
        }
        // dynamic import
        if trimmed.contains("import(") {
            if let Some(start) = trimmed.find("import(") {
                let after = &trimmed[start + 7..];
                if let Some(end) = after.find(')') {
                    let module = clean_module(&after[..end]);
                    if !module.is_empty() {
                        imports.push(format!("import {}", module));
                    }
                }
            }
        }
    }
    imports
}

fn extract_js_items(content: &str) -> Vec<String> {
    let mut items = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();

        // function foo() or async function foo()
        if let Some(rest) = trimmed.strip_prefix("async function ") {
            let name = rest.split('(').next().unwrap_or("").trim();
            if !name.is_empty() {
                items.push(format!("async fn {}", name));
            }
        } else if let Some(rest) = trimmed.strip_prefix("function ") {
            let name = rest.split('(').next().unwrap_or("").trim();
            if !name.is_empty() {
                items.push(format!("fn {}", name));
            }
        }

        // const foo = () =>  or const foo = function()
        if let Some(rest) = trimmed.strip_prefix("export ") {
            // export function foo
            if let Some(fn_rest) = rest.strip_prefix("function ") {
                let name = fn_rest.split('(').next().unwrap_or("").trim();
                if !name.is_empty() {
                    items.push(format!("fn {}", name));
                }
            }
            // export default class Foo, export class Foo
        }

        // class Foo
        if let Some(rest) = trimmed.strip_prefix("export default class ") {
            let name = rest.split_whitespace().next().unwrap_or("");
            if !name.is_empty() {
                items.push(format!("class {}", name));
            }
        } else if let Some(rest) = trimmed.strip_prefix("export class ") {
            let name = rest.split_whitespace().next().unwrap_or("");
            if !name.is_empty() {
                items.push(format!("class {}", name));
            }
        } else if let Some(rest) = trimmed.strip_prefix("class ") {
            let name = rest.split_whitespace().next().unwrap_or("");
            if !name.is_empty() {
                items.push(format!("class {}", name));
            }
        }
    }
    items
}

// ── CSS ──────────────────────────────────────────────────────────────

fn parse_css(content: &str) -> (Vec<String>, Vec<String>) {
    let mut items = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        // CSS class selector: .foo {
        if trimmed.starts_with('.') && trimmed.contains('{') {
            let name = trimmed.split_whitespace().next().unwrap_or("").trim();
            if !name.is_empty() {
                items.push(format!("class {}", name));
            }
        }
        // CSS id selector: #foo {
        if trimmed.starts_with('#') && trimmed.contains('{') {
            let name = trimmed.split_whitespace().next().unwrap_or("").trim();
            if !name.is_empty() {
                items.push(format!("id {}", name));
            }
        }
        // @media, @keyframes, etc.
        if trimmed.starts_with('@') && trimmed.contains('{') {
            let name = trimmed.split_whitespace().next().unwrap_or("").trim();
            if !name.is_empty() {
                items.push(format!("at-rule {}", name));
            }
        }
    }
    (vec![], items)
}

// ── HTML ─────────────────────────────────────────────────────────────

fn parse_html(content: &str) -> (Vec<String>, Vec<String>) {
    let mut items = Vec::new();
    // Extract script/src links as imports
    let mut imports = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        // <script src="...">
        if let Some(start) = trimmed.find("src=\"") {
            let after = &trimmed[start + 5..];
            if let Some(end) = after.find('"') {
                let src = &after[..end];
                if !src.is_empty() {
                    imports.push(format!("script {}", src));
                }
            }
        }
        // <link href="...">
        if let Some(start) = trimmed.find("href=\"") {
            let after = &trimmed[start + 6..];
            if let Some(end) = after.find('"') {
                let href = &after[..end];
                if !href.is_empty() {
                    imports.push(format!("link {}", href));
                }
            }
        }
    }
    (imports, items)
}

// ── SQL ──────────────────────────────────────────────────────────────

fn extract_sql_symbols(content: &str) -> Vec<String> {
    let mut items = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim().to_uppercase();
        if trimmed.starts_with("CREATE TABLE")
            || trimmed.starts_with("CREATE VIEW")
            || trimmed.starts_with("CREATE INDEX")
            || trimmed.starts_with("CREATE FUNCTION")
            || trimmed.starts_with("CREATE PROCEDURE")
            || trimmed.starts_with("CREATE TRIGGER")
        {
            let name = line.split_whitespace().nth(2).unwrap_or("");
            if !name.is_empty() {
                items.push(format!("table {}", name));
            }
        }
    }
    items
}

// ── Shell ────────────────────────────────────────────────────────────

fn extract_shell_functions(content: &str) -> Vec<String> {
    let mut items = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        // function foo() { or foo() {
        if let Some(rest) = trimmed.strip_prefix("function ") {
            let name = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c| c == '(' || c == ')' || c == '{');
            if !name.is_empty() {
                items.push(format!("fn {}", name));
            }
        } else if trimmed.contains("()") && trimmed.contains('{') {
            let name = trimmed.split('(').next().unwrap_or("").trim();
            if !name.is_empty() {
                items.push(format!("fn {}", name));
            }
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_js_imports() {
        let code = r#"
import React from 'react';
import { useState } from 'react';
import './App.css';
const express = require('express');
"#;
        let (imports, _) = parse_javascript(code);
        assert!(imports.iter().any(|s| s == "import react"));
        assert!(imports.iter().any(|s| s == "import ./App.css"));
        assert!(imports.iter().any(|s| s == "import express"));
    }

    #[test]
    fn parse_js_functions() {
        let code = r#"
function greet(name) { return "Hello"; }
async function fetchData(url) { return data; }
export function formatDate(d) { return d; }
"#;
        let (_, items) = parse_javascript(code);
        assert!(items.iter().any(|s| s == "fn greet"));
        assert!(items.iter().any(|s| s == "async fn fetchData"));
        assert!(items.iter().any(|s| s == "fn formatDate"));
    }

    #[test]
    fn parse_ts_types() {
        let code = r#"
interface User { name: string; }
type Status = 'active' | 'inactive';
enum Color { Red, Green, Blue }
"#;
        let (_, items) = parse_typescript(code);
        assert!(items.iter().any(|s| s == "interface User"));
        assert!(items.iter().any(|s| s == "type Status"));
        assert!(items.iter().any(|s| s == "enum Color"));
    }

    #[test]
    fn parse_jsx_components() {
        let code = r#"
function App() { return <div>Hello</div>; }
const Header = () => <h1>Title</h1>;
"#;
        let (_, items) = parse_jsx(code);
        assert!(items.iter().any(|s| s == "component App"));
        assert!(items.iter().any(|s| s == "component Header"));
    }

    #[test]
    fn parse_css_selectors() {
        let code = r#"
.container { display: flex; }
#header { color: red; }
@media (max-width: 768px) { }
"#;
        let (_, items) = parse_css(code);
        assert!(items.iter().any(|s| s == "class .container"));
        assert!(items.iter().any(|s| s == "id #header"));
        assert!(items.iter().any(|s| s == "at-rule @media"));
    }

    #[test]
    fn parse_html_links() {
        let code =
            r#"<script src="/js/app.js"></script><link href="/css/style.css" rel="stylesheet">"#;
        let (imports, _) = parse_html(code);
        assert!(imports.iter().any(|s| s == "script /js/app.js"));
        assert!(imports.iter().any(|s| s == "link /css/style.css"));
    }

    #[test]
    fn parse_sql_tables() {
        let code = r#"
CREATE TABLE users (id INT);
CREATE VIEW active_users AS SELECT * FROM users;
"#;
        let items = extract_sql_symbols(code);
        assert!(items.iter().any(|s| s == "table users"));
        assert!(items.iter().any(|s| s == "table active_users"));
    }

    #[test]
    fn parse_shell_functions() {
        let code = r#"
function deploy() {
    echo "deploying"
}
build() {
    echo "building"
}
"#;
        let items = extract_shell_functions(code);
        assert!(items.iter().any(|s| s == "fn deploy"));
        assert!(items.iter().any(|s| s == "fn build"));
    }
}
