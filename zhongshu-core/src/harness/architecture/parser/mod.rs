use std::path::Path;

use crate::harness::architecture::index::FileIndex;

pub mod csharp;
pub mod generic;
pub mod go;
pub mod java;
pub mod python;
pub mod rust;

/// Detect programming language from file extension.
pub fn detect_language(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    match ext.as_str() {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "go" => Some("go"),
        "cs" => Some("csharp"),
        "java" => Some("java"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "ts" | "tsx" => Some("typescript"),
        "jsx" => Some("jsx"),
        "css" | "scss" | "less" | "sass" => Some("css"),
        "html" | "htm" => Some("html"),
        "json" => Some("json"),
        "toml" => Some("toml"),
        "yaml" | "yml" => Some("yaml"),
        "md" => Some("markdown"),
        "xml" | "svg" => Some("xml"),
        "sql" => Some("sql"),
        "sh" | "bash" | "zsh" => Some("shell"),
        _ => None,
    }
}

/// Parse a file and return a FileIndex, dispatching to the appropriate
/// language backend.
pub fn parse_file(path: &Path, content: &str) -> FileIndex {
    let path_buf = path.to_path_buf();
    match detect_language(path) {
        Some("rust") => rust::parse_rust(path_buf, content),
        Some("python") => python::parse_python(path_buf, content),
        Some("go") => go::parse_go(path_buf, content),
        Some("csharp") => csharp::parse_csharp(path_buf, content),
        Some("java") => java::parse_java(path_buf, content),
        _ => generic::parse_generic(path_buf, content),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detect_rust() {
        assert_eq!(detect_language(Path::new("foo.rs")), Some("rust"));
    }

    #[test]
    fn detect_csharp() {
        assert_eq!(detect_language(Path::new("Foo.cs")), Some("csharp"));
    }

    #[test]
    fn detect_python() {
        assert_eq!(detect_language(Path::new("main.py")), Some("python"));
    }

    #[test]
    fn detect_javascript() {
        assert_eq!(detect_language(Path::new("app.js")), Some("javascript"));
        assert_eq!(detect_language(Path::new("app.mjs")), Some("javascript"));
    }

    #[test]
    fn detect_typescript() {
        assert_eq!(
            detect_language(Path::new("component.ts")),
            Some("typescript")
        );
        assert_eq!(
            detect_language(Path::new("component.tsx")),
            Some("typescript")
        );
    }

    #[test]
    fn detect_html() {
        assert_eq!(detect_language(Path::new("index.html")), Some("html"));
    }

    #[test]
    fn detect_css() {
        assert_eq!(detect_language(Path::new("style.css")), Some("css"));
    }

    #[test]
    fn detect_go() {
        assert_eq!(detect_language(Path::new("main.go")), Some("go"));
    }

    #[test]
    fn detect_java() {
        assert_eq!(detect_language(Path::new("Main.java")), Some("java"));
    }
}
