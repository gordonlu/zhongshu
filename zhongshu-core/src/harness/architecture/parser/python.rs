use std::path::PathBuf;

use crate::harness::architecture::index::FileIndex;

pub fn parse_python(path: PathBuf, content: &str) -> FileIndex {
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
            let name = rest.split('#').next().unwrap_or("").trim();
            if !name.is_empty() {
                imports.push(format!("import {}", name));
            }
        } else if let Some(rest) = trimmed.strip_prefix("from ") {
            if let Some(name) = rest.split_whitespace().nth(0) {
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

        // Class: class Foo:
        if let Some(rest) = trimmed.strip_prefix("class ") {
            let name = rest
                .trim_end_matches(':')
                .split('(')
                .next()
                .unwrap_or("")
                .trim();
            if !name.is_empty() {
                items.push(format!("class {}", name));
            }
            continue;
        }

        // Function: def foo(...):
        if let Some(rest) = trimmed.strip_prefix("def ") {
            let name = rest
                .trim_end_matches(':')
                .split('(')
                .next()
                .unwrap_or("")
                .trim();
            if !name.is_empty() && !name.starts_with('_') {
                items.push(format!("fn {}", name));
            }
            continue;
        }

        // Async function: async def foo(...):
        if let Some(rest) = trimmed.strip_prefix("async def ") {
            let name = rest
                .trim_end_matches(':')
                .split('(')
                .next()
                .unwrap_or("")
                .trim();
            if !name.is_empty() {
                items.push(format!("async fn {}", name));
            }
            continue;
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_python_imports() {
        let code = r#"
import os
import sys
from datetime import datetime
from typing import Optional, List
"#;
        let index = parse_python("main.py".into(), code);
        assert!(index.imports.contains(&"import os".to_string()));
        assert!(index.imports.contains(&"import sys".to_string()));
        assert!(index.imports.contains(&"import datetime".to_string()));
    }

    #[test]
    fn parse_python_functions() {
        let code = r#"
def hello():
    pass

async def fetch_data():
    pass

class MyClass:
    def method(self):
        pass
"#;
        let index = parse_python("main.py".into(), code);
        assert!(index.items.iter().any(|s| s == "fn hello"));
        assert!(index.items.iter().any(|s| s == "async fn fetch_data"));
        assert!(index.items.iter().any(|s| s == "class MyClass"));
    }
}
