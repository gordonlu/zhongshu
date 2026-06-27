use std::path::Path;

use crate::harness::action::{FeedbackSource, HarnessFeedback, Severity};
use crate::harness::architecture::config::{ArchitectureRule, SemanticMatcher};

/// Check file content for semantic issues based on configured rules.
pub fn check_semantics(
    content: &str,
    file: &Path,
    rules: &[ArchitectureRule],
) -> Vec<HarnessFeedback> {
    let mut feedback = Vec::new();

    for rule in rules {
        let ArchitectureRule::SemanticRule {
            name,
            matcher,
            severity,
        } = rule
        else {
            continue;
        };

        match matcher {
            SemanticMatcher::ForbidIgnoredResult {
                file_globs,
                callee_contains,
            } => {
                if !matches_globs(file, file_globs) {
                    continue;
                }
                let findings = check_ignored_result(content, callee_contains);
                if !findings.is_empty() {
                    feedback.push(HarnessFeedback {
                        source: FeedbackSource::Architecture,
                        severity: *severity,
                        rule_id: name.clone(),
                        message: format!(
                            "{} 处调用的返回结果可能被忽略",
                            findings.join(", ")
                        ),
                        suggestion: "使用 `let _ = ...` 或 `.await?` 处理返回结果".into(),
                        evidence: Some(format!("文件：{}", file.display())),
                    });
                }
            }
            SemanticMatcher::ForbidPattern {
                file_globs,
                patterns,
            } => {
                if !matches_globs(file, file_globs) {
                    continue;
                }
                let findings: Vec<&str> = patterns
                    .iter()
                    .filter(|p| content.contains(p.as_str()))
                    .map(|p| p.as_str())
                    .collect();
                if !findings.is_empty() {
                    feedback.push(HarnessFeedback {
                        source: FeedbackSource::Architecture,
                        severity: *severity,
                        rule_id: name.clone(),
                        message: format!(
                            "检测到禁止的模式: {}",
                            findings.join(", ")
                        ),
                        suggestion: format!(
                            "文件 {} 中不允许使用模式: {}",
                            file.display(),
                            findings.join(", ")
                        ),
                        evidence: Some(format!("文件：{}", file.display())),
                    });
                }
            }
            SemanticMatcher::AllowIgnoredResult { .. } => {
                // Allow-list is handled by check_ignored_result — skip here
            }
        }
    }

    feedback
}

fn matches_globs(file: &Path, globs: &[globset::GlobMatcher]) -> bool {
    if globs.is_empty() {
        return true;
    }
    globs.iter().any(|g| g.is_match(file))
}

/// Check for call patterns where the result might be accidentally ignored.
fn check_ignored_result(content: &str, callee_contains: &[String]) -> Vec<String> {
    let mut findings = Vec::new();
    let lines: Vec<&str> = content.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        // Match lines that end with a call to the target but no result handling
        let has_semi = trimmed.ends_with(';');
        let has_await_semi = trimmed.ends_with(".await;");
        let has_question = trimmed.ends_with(".await?;") || trimmed.ends_with("?;");

        if !has_semi || has_await_semi || has_question {
            continue;
        }

        let has_callee = callee_contains.iter().any(|cc| {
            trimmed
                .to_lowercase()
                .contains(&cc.to_lowercase())
        });

        if has_callee {
            let is_handled = trimmed.starts_with("let ")
                || trimmed.starts_with("let _")
                || trimmed.starts_with("let mut")
                || trimmed.starts_with("if let")
                || trimmed.starts_with("while let")
                || trimmed.starts_with("match")
                || trimmed.contains(".await?")
                || trimmed.contains("?;")
                || trimmed.contains("as Result<")
                || trimmed.contains("as Option<");

            if !is_handled {
                findings.push(format!("第 {} 行: {}", i + 1, trimmed));
            }
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::architecture::config::SemanticMatcher;

    fn make_forbid_pattern_rule(
        name: &str,
        glob: &str,
        pattern: &str,
        severity: Severity,
    ) -> ArchitectureRule {
        ArchitectureRule::SemanticRule {
            name: name.into(),
            matcher: SemanticMatcher::ForbidPattern {
                file_globs: vec![globset::Glob::new(glob)
                    .unwrap()
                    .compile_matcher()],
                patterns: vec![pattern.into()],
            },
            severity,
        }
    }

    #[test]
    fn forbid_pattern_detects_violation() {
        let rules = vec![make_forbid_pattern_rule(
            "no_todo",
            "*.rs",
            "TODO",
            Severity::Warning,
        )];
        let fb = check_semantics("fn foo() { /* TODO */ }", Path::new("test.rs"), &rules);
        assert!(!fb.is_empty());
        assert!(fb[0].message.contains("TODO"));
    }

    #[test]
    fn forbid_pattern_respects_glob() {
        let rules = vec![make_forbid_pattern_rule(
            "no_todo_in_rs",
            "*.py",
            "TODO",
            Severity::Warning,
        )];
        let fb = check_semantics("fn foo() { /* TODO */ }", Path::new("test.rs"), &rules);
        assert!(fb.is_empty());
    }

    #[test]
    fn clean_file_no_violations() {
        let rules = vec![make_forbid_pattern_rule(
            "no_todo",
            "*.rs",
            "TODO",
            Severity::Warning,
        )];
        let fb = check_semantics(
            "fn foo() { /* done */ }",
            Path::new("test.rs"),
            &rules,
        );
        assert!(fb.is_empty());
    }

    #[test]
    fn check_ignored_result_detects_unhandled_call() {
        let content = "db.save(record);";
        let findings = check_ignored_result(content, &["db.save".into()]);
        assert!(!findings.is_empty());
        assert!(findings[0].contains("第 1 行"));
    }

    #[test]
    fn check_ignored_result_skips_let() {
        let content = "let result = db.save(record);";
        let findings = check_ignored_result(content, &["db.save".into()]);
        assert!(findings.is_empty());
    }

    #[test]
    fn check_ignored_result_skips_question_mark() {
        let content = "db.save(record).await?;";
        let findings = check_ignored_result(content, &["db.save".into()]);
        assert!(findings.is_empty());
    }
}
