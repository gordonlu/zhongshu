use std::path::PathBuf;

use crate::harness::architecture::index::FileIndex;

pub fn parse_go(path: PathBuf, content: &str) -> FileIndex {
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
    let mut in_import_block = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "import (" {
            in_import_block = true;
            continue;
        }
        if trimmed == ")" {
            in_import_block = false;
            continue;
        }
        if in_import_block {
            let name = trimmed.trim_matches('"');
            if !name.is_empty() {
                imports.push(format!("import {}", name));
            }
        }
        if let Some(rest) = trimmed.strip_prefix("import ") {
            if !rest.starts_with('(') {
                let name = rest.trim_matches('"');
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

        // func Foo(...): public function
        if let Some(rest) = trimmed.strip_prefix("func ") {
            let name = rest.split('(').next().unwrap_or("").trim();
            if !name.is_empty() {
                items.push(format!("fn {}", name));
            }
            continue;
        }

        // type Foo struct / type Foo interface
        if let Some(rest) = trimmed.strip_prefix("type ") {
            let name = rest.split_whitespace().next().unwrap_or("");
            if !name.is_empty() {
                items.push(format!("type {}", name));
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
    fn parse_go_imports() {
        let code = r#"
import (
    "fmt"
    "net/http"
)
import "strings"
"#;
        let index = parse_go("main.go".into(), code);
        assert!(index.imports.contains(&"import fmt".to_string()));
        assert!(index.imports.contains(&"import net/http".to_string()));
        assert!(index.imports.contains(&"import strings".to_string()));
    }

    #[test]
    fn parse_go_functions() {
        let code = r#"
func main() {}
func Hello(w http.ResponseWriter, r *http.Request) {}
"#;
        let index = parse_go("main.go".into(), code);
        assert!(index.items.iter().any(|s| s == "fn main"));
        assert!(index.items.iter().any(|s| s == "fn Hello"));
    }

    #[test]
    fn parse_go_types() {
        let code = r#"
type User struct {
    Name string
}
type Handler interface {
    ServeHTTP(w http.ResponseWriter, r *http.Request)
}
"#;
        let index = parse_go("types.go".into(), code);
        assert!(index.items.iter().any(|s| s == "type User"));
        assert!(index.items.iter().any(|s| s == "type Handler"));
    }
}
