//! Static layer-boundary checks for the Zhongshu workspace.
//!
//! Unlike `super::rules`, which runs at runtime inside the coding harness
//! for LLM feedback, this module provides a deterministic build-time scan
//! run from `cargo xtask proof` to block PRs with architecture regressions.
//!
//! # Rules
//! - B001: core → orb (core must never depend on orb)
//! - B002: orb → core::db (direct db module penetration)
//! - B003: orb → `Database` (owning a SQLite handle from orb)
//! - B004: orb → `*Repository` / `*Store` (persistence types from orb)

use std::fmt::Write as FmtWrite;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub rule: &'static str,
    pub file: String,
    pub line_no: usize,
    pub detail: String,
    pub message: String,
}

type BaselineEntry = (&'static str, &'static str);

/// Pre-existing violations that are tracked but do not block CI.
/// Keyed by (filename suffix, violating type name).
const BASELINE: &[BaselineEntry] = &[
    ("main.rs", "Database"),
    ("main.rs", "EventLogStore"),
    ("main.rs", "GoalRepository"),
    ("main.rs", "MemoryCandidateStore"),
    ("main.rs", "ObservationStore"),
    ("main.rs", "RunbookStore"),
    ("main.rs", "TaskRepository"),
    ("app.rs", "Database"),
    ("app.rs", "RunbookStore"),
    ("services.rs", "ArtifactRepository"),
    ("services.rs", "Database"),
    ("services.rs", "GoalRepository"),
    ("services.rs", "MemoryCandidateStore"),
    ("services.rs", "ObservationStore"),
    ("services.rs", "RunbookStore"),
    ("services.rs", "TaskRepository"),
    ("handler.rs", "TaskRepository"),
    ("handler.rs", "RunbookStore"),
];

fn is_baseline(file: &str, detail: &str) -> bool {
    BASELINE
        .iter()
        .any(|(f, d)| file.ends_with(f) && detail == *d)
}

/// Run the full boundary check and return violations.
pub fn check_boundaries(workspace: &Path) -> Vec<Violation> {
    let mut violations = Vec::new();
    let orb_dir = workspace.join("zhongshu-orb").join("src");
    let core_dir = workspace.join("zhongshu-core").join("src");

    // Pass 1: scan use statements
    if orb_dir.exists() {
        scan_use_statements(&orb_dir, "zhongshu-orb/src", &mut violations);
    }
    if core_dir.exists() {
        scan_use_statements(&core_dir, "zhongshu-core/src", &mut violations);
    }

    // Pass 2: scan inline fully-qualified paths
    if orb_dir.exists() {
        scan_inline_fqn(&orb_dir, "zhongshu-orb/src", &mut violations);
    }
    if core_dir.exists() {
        scan_inline_fqn(&core_dir, "zhongshu-core/src", &mut violations);
    }

    violations
}

/// Pass 1: extract `use` statements and check against rules.
fn scan_use_statements(dir: &Path, prefix: &str, violations: &mut Vec<Violation>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_use_statements(&path, prefix, violations);
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let rel = path.to_string_lossy().to_string();
        let rel = if let Some(p) = rel.split_once(prefix) {
            p.1.trim_start_matches('/')
        } else {
            continue;
        };
        let rel_path = format!("{prefix}/{rel}");
        let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");

        for (i, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("use ") || trimmed.starts_with("pub use ") {
                let use_line = trimmed.strip_prefix("pub ").unwrap_or(trimmed);
                if use_line.starts_with("use crate") || use_line.starts_with("use super") {
                    continue;
                }

                // Handle multi-line grouped imports: if line ends with '{'
                if trimmed.contains('{') && !trimmed.contains('}') {
                    let mut block = trimmed.to_string();
                    for (j, next_line) in content.lines().enumerate().skip(i + 1) {
                        let next_trimmed = next_line.trim();
                        // Preserve whitespace before the closing brace
                        block.push_str(next_line);
                        if next_trimmed.contains('}') || next_trimmed.ends_with(';') {
                            break;
                        }
                        if j > i + 50 {
                            break;
                        }
                    }
                    expand_imports(&block, &rel_path, file_name, i + 1, violations);
                } else {
                    expand_imports(use_line, &rel_path, file_name, i + 1, violations);
                }
            }
        }
    }
}

/// Expand a use statement into individual import paths and check each.
fn expand_imports(
    text: &str,
    rel_path: &str,
    file_name: &str,
    line: usize,
    violations: &mut Vec<Violation>,
) {
    let text = text.strip_suffix(';').unwrap_or(text);
    let text = text.strip_prefix("use ").unwrap_or(text);
    let text = text.strip_prefix("pub use ").unwrap_or(text);

    // Extract the crate prefix and check for grouped imports
    let parts: Vec<&str> = text.splitn(2, "::").collect();
    if parts.len() < 2 {
        // Bare crate import: `use zhongshu_orb;`
        let bare = parts[0].trim();
        if bare == "zhongshu_orb" {
            violations.push(Violation {
                rule: "B001",
                file: rel_path.into(),
                line_no: line,
                detail: bare.into(),
                message: "core must not depend on orb (bare crate import)".into(),
            });
        }
        return;
    }
    let prefix = parts[0].trim();
    let rest = parts[1];

    if prefix == "zhongshu_core" && rel_path.starts_with("zhongshu-core") {
        if rest.contains("zhongshu_orb") || rest.contains("orb") {
            // Check for zhongshu_orb paths
            if let Some(_idx) = rest.find("zhongshu_orb") {
                violations.push(Violation {
                    rule: "B001",
                    file: rel_path.into(),
                    line_no: line,
                    detail: format!("{prefix}::{rest}"),
                    message: "core must not depend on orb layer".into(),
                });
            }
        }
    }

    if prefix == "zhongshu_orb" {
        if rel_path.starts_with("zhongshu-core") {
            // B001: core using orb
            violations.push(Violation {
                rule: "B001",
                file: rel_path.into(),
                line_no: line,
                detail: format!("{prefix}::{rest}"),
                message: "core must not depend on orb layer".into(),
            });
        }
        return; // orb→orb is fine, skip further checks
    }

    if prefix == "zhongshu_core" && rel_path.starts_with("zhongshu-orb") {
        // Extracted individual types from grouped imports
        let types = extract_types_from_use_path(rest);
        for ty in &types {
            check_orb_to_core(ty, rel_path, file_name, line, violations);
        }
    }
}

/// Extract type names from a use path, handling grouped `{A, B, C}` syntax.
fn extract_types_from_use_path(path: &str) -> Vec<String> {
    let mut result = Vec::new();
    if let Some(inner) = path.find('{') {
        // Grouped: zhongshu_core::core::{Database, TaskRepository}
        let base = &path[..inner].trim_end_matches("::");
        let body = &path[inner..];
        let body = body.strip_prefix('{').unwrap_or(body);
        let body = body.strip_suffix('}').unwrap_or(body);
        for item in body.split(',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            if item.contains("::") {
                result.push(format!("{base}::{item}"));
            } else {
                result.push(item.to_string());
            }
        }
    } else {
        result.push(path.to_string());
    }
    result
}

/// Check a single orb-side import against B002/B003/B004.
fn check_orb_to_core(
    ty: &str,
    rel_path: &str,
    file_name: &str,
    line: usize,
    violations: &mut Vec<Violation>,
) {
    let ty_lower = ty.to_lowercase();

    // B002: direct db module
    if ty_lower.contains("::db::") || ty_lower.starts_with("db::") {
        if !is_baseline(file_name, ty) {
            violations.push(Violation {
                rule: "B002",
                file: rel_path.into(),
                line_no: line,
                detail: ty.into(),
                message: "orb must not directly import core db module".into(),
            });
        }
        return;
    }

    // B003: concrete Database type
    if ty == "Database" || ty.ends_with("::Database") {
        if !is_baseline(file_name, ty) {
            violations.push(Violation {
                rule: "B003",
                file: rel_path.into(),
                line_no: line,
                detail: ty.into(),
                message: "orb must not own a Database handle; use EventBus or service traits"
                    .into(),
            });
        }
        return;
    }

    // B004: *Repository / *Store types from zhongshu_core::core
    // Skip Tool types, which are public API
    let ident = ty.split("::").last().unwrap_or(ty);
    if (ident.ends_with("Repository") || ident.ends_with("Store"))
        && !ident.ends_with("Tool")
        && !ident.ends_with("Registry")
    {
        if !is_baseline(file_name, ident) {
            violations.push(Violation {
                rule: "B004",
                file: rel_path.into(),
                line_no: line,
                detail: ident.into(),
                message: format!("orb must not directly use core storage type `{ident}`; use EventBus or service traits"),
            });
        }
    }
}

/// Pass 2: scan for inline fully-qualified paths (e.g. `zhongshu_core::core::TaskRepository::new()`).
fn scan_inline_fqn(dir: &Path, prefix: &str, violations: &mut Vec<Violation>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_inline_fqn(&path, prefix, violations);
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let rel = path.to_string_lossy().to_string();
        let rel = if let Some(p) = rel.split_once(prefix) {
            p.1.trim_start_matches('/')
        } else {
            continue;
        };
        let rel_path = format!("{prefix}/{rel}");
        let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");

        let stripped = strip_comments_and_strings(&content);

        for (i, line) in stripped.lines().enumerate() {
            let line = line.trim();
            // Skip use statements (handled in Pass 1)
            if line.starts_with("use ") || line.starts_with("pub use ") {
                continue;
            }
            // Check for B001: zhongshu_orb:: in core
            if rel_path.starts_with("zhongshu-core") && line.contains("zhongshu_orb::") {
                let detail = extract_fqn_path(line, "zhongshu_orb");
                if !detail.is_empty() {
                    violations.push(Violation {
                        rule: "B001",
                        file: rel_path.clone(),
                        line_no: i + 1,
                        detail,
                        message: "core must not depend on orb (inline fully-qualified path)".into(),
                    });
                }
            }
            // Check for B002/B003/B004: zhongshu_core::core::* in orb
            if rel_path.starts_with("zhongshu-orb") && line.contains("zhongshu_core::core::") {
                let fqn = extract_fqn_path(line, "zhongshu_core");
                if fqn.is_empty() {
                    continue;
                }
                let ident = fqn.split("::").last().unwrap_or(&fqn);
                // Skip public API: EventBus, AgentRuntime, ToolRegistry, etc.
                let public_apis = [
                    "EventBus",
                    "AgentRuntime",
                    "ToolRegistry",
                    "AuthorityGate",
                    "Event",
                    "MessageId",
                    "ResponseEvent",
                    "ResponseRole",
                ];
                if public_apis.contains(&ident)
                    || ident.contains("context::")
                    || ident.ends_with("Tool")
                {
                    continue;
                }
                if ident == "Database" {
                    if !is_baseline(file_name, ident) {
                        violations.push(Violation {
                            rule: "B003",
                            file: rel_path.clone(),
                            line_no: i + 1,
                            detail: ident.into(),
                            message:
                                "orb must not own a Database handle; use EventBus or service traits"
                                    .into(),
                        });
                    }
                } else if (ident.ends_with("Repository") || ident.ends_with("Store"))
                    && !ident.ends_with("Tool")
                {
                    if !is_baseline(file_name, ident) {
                        violations.push(Violation {
                            rule: "B004",
                            file: rel_path.clone(),
                            line_no: i + 1,
                            detail: ident.into(),
                            message: format!(
                                "orb must not directly use core storage type `{ident}`"
                            ),
                        });
                    }
                }
            }
        }
    }
}

/// Extract a fully-qualified path starting with `crate_name` from a line.
fn extract_fqn_path(line: &str, crate_name: &str) -> String {
    let idx = match line.find(crate_name) {
        Some(i) => i,
        None => return String::new(),
    };
    let rest = &line[idx..];
    let mut path = String::new();
    for ch in rest.chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == ':' {
            path.push(ch);
        } else {
            break;
        }
    }
    // Normalize `::` while keeping at least the crate name
    if path.matches("::").count() >= 1 {
        path
    } else {
        String::new()
    }
}

/// Strip comments and string contents to avoid false positives.
fn strip_comments_and_strings(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Line comment: //
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            let end = content[i..]
                .find('\n')
                .map(|p| i + p + 1)
                .unwrap_or(content.len());
            // Preserve the newline to keep line numbers
            result.push_str(&" ".repeat(end - i - 1));
            if end < content.len() {
                result.push('\n');
            }
            i = end;
            continue;
        }
        // Block comment: /* ... */
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            let end = content[i..]
                .find("*/")
                .map(|p| i + p + 2)
                .unwrap_or(content.len());
            // Count newlines to keep line numbers
            let comment = &content[i..end];
            let nl_count = comment.bytes().filter(|&b| b == b'\n').count();
            for _ in 0..nl_count {
                result.push('\n');
            }
            i = end;
            continue;
        }
        // String literal: "..." (handle escaped quotes)
        if bytes[i] == b'"' {
            let mut j = i + 1;
            while j < bytes.len() {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2; // skip escaped char
                    continue;
                }
                if bytes[j] == b'"' {
                    j += 1;
                    break;
                }
                j += 1;
            }
            // Replace string content with spaces
            let len = j - i;
            result.push_str(&" ".repeat(len));
            i = j;
            continue;
        }
        // Raw string: r#"..."#
        if i + 1 < bytes.len() && bytes[i] == b'r' && bytes[i + 1] == b'"' {
            let mut hash_count = 0;
            let mut j = i + 2;
            while j < bytes.len() && bytes[j] == b'#' {
                hash_count += 1;
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'"' {
                j += 1;
                let end_tag: Vec<u8> = std::iter::once(b'"')
                    .chain(std::iter::repeat(b'#').take(hash_count))
                    .collect();
                let end = content[j..]
                    .find(std::str::from_utf8(&end_tag).unwrap_or("\""))
                    .map(|p| j + p + end_tag.len())
                    .unwrap_or(content.len());
                let section = &content[i..end];
                let nl_count = section.bytes().filter(|&b| b == b'\n').count();
                for _ in 0..nl_count {
                    result.push('\n');
                }
                i = end;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

pub fn format_report(violations: &[Violation]) -> String {
    if violations.is_empty() {
        return "architecture boundary check: 0 violations".into();
    }
    let baseline_count = violations
        .iter()
        .filter(|v| is_baseline(&v.file, &v.detail))
        .count();
    let new_count = violations.len() - baseline_count;
    let mut out = format!(
        "architecture boundary check: {} violations ({} new, {} baseline)\n",
        violations.len(),
        new_count,
        baseline_count,
    );
    for v in violations {
        let tag = if is_baseline(&v.file, &v.detail) {
            "[BASELINE]"
        } else {
            "[NEW]"
        };
        writeln!(
            &mut out,
            "  {} {}:{}  {}  {}",
            tag, v.file, v.line_no, v.rule, v.message
        )
        .unwrap();
    }
    out
}

pub struct BoundaryResult {
    pub ok: bool,
    pub report: String,
}

pub fn check_workspace_boundaries(workspace: &Path) -> BoundaryResult {
    let violations = check_boundaries(workspace);
    let new_violations: Vec<&Violation> = violations
        .iter()
        .filter(|v| !is_baseline(&v.file, &v.detail))
        .collect();
    let report = format_report(&violations);
    BoundaryResult {
        ok: new_violations.is_empty(),
        report,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_core_use_of_orb() {
        let content = "use zhongshu_orb::Overlay;";
        let mut v = Vec::new();
        expand_imports(content, "zhongshu-core/src/foo.rs", "foo.rs", 1, &mut v);
        assert!(
            v.iter().any(|x| x.rule == "B001"),
            "should detect B001: {v:?}"
        );
    }

    #[test]
    fn detects_orb_use_of_database() {
        let mut v = Vec::new();
        expand_imports(
            "zhongshu_core::core::Database",
            "zhongshu-orb/src/main.rs",
            "main.rs",
            1,
            &mut v,
        );
        assert!(v.iter().any(|x| x.rule == "B003"));
    }

    #[test]
    fn detects_orb_use_of_repository() {
        let mut v = Vec::new();
        expand_imports(
            "zhongshu_core::core::NewVendorRepository",
            "zhongshu-orb/src/main.rs",
            "main.rs",
            1,
            &mut v,
        );
        assert!(v.iter().any(|x| x.rule == "B004"));
    }

    #[test]
    fn does_not_flag_public_api() {
        let mut v = Vec::new();
        for api in &[
            "EventBus",
            "AgentRuntime",
            "ToolRegistry",
            "AuthorityGate",
            "TaskTool",
            "GoalTool",
        ] {
            v.clear();
            expand_imports(
                &format!("zhongshu_core::core::{api}"),
                "zhongshu-orb/src/main.rs",
                "other.rs",
                1,
                &mut v,
            );
            assert!(
                !v.iter().any(|x| x.rule == "B004"),
                "should not flag {api}: {v:?}"
            );
        }
    }

    #[test]
    fn baseline_suppresses_existing_violations() {
        assert!(is_baseline("main.rs", "Database"));
        assert!(is_baseline("services.rs", "TaskRepository"));
        assert!(is_baseline("handler.rs", "TaskRepository"));
        assert!(!is_baseline("main.rs", "SomeNewType"));
        assert!(!is_baseline("new_file.rs", "Database"));
    }

    #[test]
    fn inline_fqn_detection() {
        let stripped =
            strip_comments_and_strings("fn foo(x: zhongshu_core::core::Database) {} // comment");
        let cleaned = stripped.trim_start();
        assert!(cleaned.starts_with("fn foo(x: zhongshu_core::core::Database)"));
    }

    #[test]
    fn strip_comments_removes_line_comment() {
        let result = strip_comments_and_strings("foo\n// bar\nbaz");
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].trim(), "foo");
        assert_eq!(lines[2].trim(), "baz");
    }

    #[test]
    fn strip_strings_removes_string_content() {
        let result = strip_comments_and_strings("let x = \"hello zhongshu_orb::world\";");
        // The string content (including zhongshu_orb::) should be replaced with spaces
        assert!(
            !result.contains("zhongshu_orb::"),
            "string content not stripped"
        );
    }

    #[test]
    fn extract_types_from_grouped_import() {
        let types = extract_types_from_use_path("core::{Database, TaskRepository, RunbookStore}");
        assert!(types.contains(&"Database".to_string()));
        assert!(types.contains(&"TaskRepository".to_string()));
        assert!(types.contains(&"RunbookStore".to_string()));
    }

    #[test]
    fn protocol_types_are_not_flagged() {
        let mut v = Vec::new();
        // EventBus/Event are under zhongshu_core::event not zhongshu_core::core::
        // But even if under core, should not be flagged
        let safe = [
            "context::ContextMessage",
            "MemoryPolicy",
            "Scheduler",
            "SuggestionEngine",
        ];
        for api in &safe {
            v.clear();
            expand_imports(
                &format!("zhongshu_core::core::{api}"),
                "zhongshu-orb/src/services.rs",
                "services.rs",
                1,
                &mut v,
            );
            // Check that it's not flagged as B004
            // MemoryPolicy/Scheduler/SuggestionEngine are not Repository/Store
            if !api.ends_with("Repository") && !api.ends_with("Store") {
                assert!(
                    !v.iter().any(|x| x.rule == "B004"),
                    "should not flag {api}: {v:?}"
                );
            }
        }
    }

    #[test]
    fn does_not_flag_equipment_registry() {
        let mut v = Vec::new();
        expand_imports(
            "zhongshu_core::equipment::EquipmentRegistry",
            "zhongshu-orb/src/main.rs",
            "main.rs",
            1,
            &mut v,
        );
        assert!(
            !v.iter().any(|x| x.rule == "B004"),
            "equipment should not be storage: {v:?}"
        );
    }

    #[test]
    fn new_file_not_covered_by_baseline() {
        let mut v = Vec::new();
        expand_imports(
            "zhongshu_core::core::Database",
            "zhongshu-orb/src/editor.rs",
            "editor.rs",
            1,
            &mut v,
        );
        assert!(
            v.iter().any(|x| x.rule == "B003"),
            "new file should be caught"
        );
    }

    #[test]
    fn new_type_in_baseline_file_is_not_suppressed() {
        // A type not in baseline should still fire even if the file is baselined
        let mut v = Vec::new();
        expand_imports(
            "zhongshu_core::core::NewVendorRepository",
            "zhongshu-orb/src/main.rs",
            "main.rs",
            1,
            &mut v,
        );
        assert!(
            v.iter().any(|x| x.rule == "B004"),
            "new type in baselined file should fire"
        );
    }
}
