use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::harness::architecture::index::ProjectIndex;
use crate::harness::verification::plan::{VerificationCommand, VerificationPlan};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoIntelligenceReport {
    pub changed_files: Vec<PathBuf>,
    pub affected_files: Vec<PathBuf>,
    pub affected_symbols: Vec<String>,
    pub risks: Vec<RepoRisk>,
    pub verification: VerificationPlan,
    pub working_set: WorkingSet,
}

impl RepoIntelligenceReport {
    pub fn for_changes(
        index: &ProjectIndex,
        changed_files: &[PathBuf],
        task_description: &str,
        tracker: &WorkingSetTracker,
    ) -> Self {
        let changed_files = normalize_files(changed_files);
        let affected_symbols = affected_symbols(index, &changed_files);
        let affected_files = affected_files(index, &changed_files, &affected_symbols);
        let risks = changed_files
            .iter()
            .flat_map(|path| risks_for_path(path))
            .collect();
        let verification = VerificationPlan::for_changes(&changed_files, task_description);

        Self {
            changed_files,
            affected_files,
            affected_symbols,
            risks,
            verification,
            working_set: tracker.snapshot(),
        }
    }

    pub fn recommended_commands(&self) -> Vec<VerificationCommand> {
        self.verification.commands.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoRisk {
    pub path: PathBuf,
    pub kind: RepoRiskKind,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoRiskKind {
    Generated,
    Migration,
    BuildScript,
    PlatformSpecific,
    Lockfile,
    Config,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingSet {
    pub files: Vec<WorkingSetFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingSetFile {
    pub path: PathBuf,
    pub reads: u32,
    pub edits: u32,
    pub test_failures: u32,
    pub architecture_violations: u32,
    pub score: u32,
    pub last_reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct WorkingSetTracker {
    files: BTreeMap<PathBuf, WorkingSetFile>,
}

impl WorkingSetTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, signal: WorkingSetSignal) {
        match signal {
            WorkingSetSignal::FileRead { path } => {
                self.entry(path, "file read").reads += 1;
            }
            WorkingSetSignal::FileEdit { path } => {
                self.entry(path, "file edit").edits += 1;
            }
            WorkingSetSignal::TestFailure { path, command } => {
                let entry = self.entry(path, format!("test failed: {command}"));
                entry.test_failures += 1;
            }
            WorkingSetSignal::ArchitectureViolation { path, rule_id } => {
                let entry = self.entry(path, format!("architecture violation: {rule_id}"));
                entry.architecture_violations += 1;
            }
        }
    }

    pub fn snapshot(&self) -> WorkingSet {
        let mut files: Vec<_> = self.files.values().cloned().collect();
        for file in &mut files {
            file.score = working_set_score(file);
        }
        files.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
        WorkingSet { files }
    }

    fn entry(&mut self, path: PathBuf, reason: impl Into<String>) -> &mut WorkingSetFile {
        let path = normalize_path(&path);
        let reason = reason.into();
        let entry = self
            .files
            .entry(path.clone())
            .or_insert_with(|| WorkingSetFile {
                path,
                reads: 0,
                edits: 0,
                test_failures: 0,
                architecture_violations: 0,
                score: 0,
                last_reason: String::new(),
            });
        entry.last_reason = reason;
        entry
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkingSetSignal {
    FileRead { path: PathBuf },
    FileEdit { path: PathBuf },
    TestFailure { path: PathBuf, command: String },
    ArchitectureViolation { path: PathBuf, rule_id: String },
}

fn affected_symbols(index: &ProjectIndex, changed_files: &[PathBuf]) -> Vec<String> {
    let changed: BTreeSet<_> = changed_files
        .iter()
        .map(|path| normalize_path(path))
        .collect();
    let mut symbols = BTreeSet::new();
    for (path, file_index) in &index.files {
        if changed.contains(&normalize_path(path)) {
            for item in &file_index.items {
                symbols.insert(item.clone());
            }
        }
    }
    symbols.into_iter().collect()
}

fn affected_files(
    index: &ProjectIndex,
    changed_files: &[PathBuf],
    affected_symbols: &[String],
) -> Vec<PathBuf> {
    let mut files: BTreeSet<PathBuf> = changed_files
        .iter()
        .map(|path| normalize_path(path))
        .collect();
    let changed_stems: BTreeSet<String> = changed_files
        .iter()
        .filter_map(|path| path.file_stem().and_then(|stem| stem.to_str()))
        .map(|stem| stem.to_lowercase())
        .collect();
    let short_symbols: BTreeSet<String> = affected_symbols
        .iter()
        .map(|symbol| symbol_short_name(symbol).to_lowercase())
        .collect();

    for (path, file_index) in &index.files {
        let normalized = normalize_path(path);
        if files.contains(&normalized) {
            continue;
        }
        if file_index.imports.iter().any(|import| {
            let import = import.to_lowercase();
            changed_stems.iter().any(|stem| import.contains(stem))
                || short_symbols.iter().any(|symbol| import.contains(symbol))
        }) {
            files.insert(normalized);
        }
    }

    files.into_iter().collect()
}

fn risks_for_path(path: &Path) -> Vec<RepoRisk> {
    let normalized = normalize_path(path);
    let lower = normalized
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase();
    let mut risks = Vec::new();

    if lower.contains("/generated/")
        || lower.ends_with(".generated.rs")
        || lower.ends_with(".g.rs")
        || lower.ends_with(".pb.rs")
    {
        risks.push(RepoRisk {
            path: normalized.clone(),
            kind: RepoRiskKind::Generated,
            reason: "generated files should usually be changed at their source".into(),
        });
    }
    if lower.contains("/migration")
        || lower.contains("/migrations/")
        || lower.contains("/schema/")
        || lower.ends_with(".sql")
    {
        risks.push(RepoRisk {
            path: normalized.clone(),
            kind: RepoRiskKind::Migration,
            reason: "migration and schema changes can affect persisted state".into(),
        });
    }
    if normalized.file_name().and_then(|name| name.to_str()) == Some("build.rs") {
        risks.push(RepoRisk {
            path: normalized.clone(),
            kind: RepoRiskKind::BuildScript,
            reason: "build scripts can affect compile-time behavior".into(),
        });
    }
    if lower.contains("/windows/")
        || lower.contains("/linux/")
        || lower.contains("/macos/")
        || lower.contains("target_os")
        || lower.contains("overlay_windows")
    {
        risks.push(RepoRisk {
            path: normalized.clone(),
            kind: RepoRiskKind::PlatformSpecific,
            reason: "platform-specific files require platform-aware validation".into(),
        });
    }
    if matches!(
        normalized.file_name().and_then(|name| name.to_str()),
        Some("Cargo.lock" | "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock")
    ) {
        risks.push(RepoRisk {
            path: normalized.clone(),
            kind: RepoRiskKind::Lockfile,
            reason: "lockfile changes can alter dependency resolution".into(),
        });
    }
    if matches!(
        normalized.extension().and_then(|ext| ext.to_str()),
        Some("toml" | "json" | "yaml" | "yml")
    ) {
        risks.push(RepoRisk {
            path: normalized,
            kind: RepoRiskKind::Config,
            reason: "configuration changes need behavior validation".into(),
        });
    }

    risks
}

fn normalize_files(files: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<_> = files.iter().map(|path| normalize_path(path)).collect();
    out.sort();
    out.dedup();
    out
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn symbol_short_name(symbol: &str) -> &str {
    symbol
        .strip_prefix("fn ")
        .or_else(|| symbol.strip_prefix("struct "))
        .or_else(|| symbol.strip_prefix("enum "))
        .or_else(|| symbol.strip_prefix("trait "))
        .unwrap_or(symbol)
}

fn working_set_score(file: &WorkingSetFile) -> u32 {
    file.reads + file.edits * 4 + file.test_failures * 6 + file.architecture_violations * 8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::architecture::index::{FileIndex, ProjectIndex};

    fn index_with(files: Vec<(&str, Vec<&str>, Vec<&str>)>) -> ProjectIndex {
        let mut index = ProjectIndex::new(PathBuf::from("."));
        for (path, imports, items) in files {
            let path = PathBuf::from(path);
            index.files.insert(
                path.clone(),
                FileIndex {
                    path,
                    imports: imports.into_iter().map(ToString::to_string).collect(),
                    items: items.into_iter().map(ToString::to_string).collect(),
                    parse_error: None,
                },
            );
        }
        index
    }

    #[test]
    fn reports_symbols_and_import_referrers_for_changed_file() {
        let index = index_with(vec![
            (
                "src/core/session.rs",
                vec![],
                vec!["struct CodingSession", "fn start_session"],
            ),
            (
                "src/agent/loop.rs",
                vec!["crate::core::session::CodingSession"],
                vec!["fn run"],
            ),
        ]);
        let tracker = WorkingSetTracker::new();

        let report = RepoIntelligenceReport::for_changes(
            &index,
            &[PathBuf::from("src/core/session.rs")],
            "implement coding session",
            &tracker,
        );

        assert!(report
            .affected_symbols
            .contains(&"struct CodingSession".to_string()));
        assert!(report
            .affected_files
            .contains(&PathBuf::from("src/agent/loop.rs")));
        assert!(report.verification.required);
    }

    #[test]
    fn risk_map_marks_build_scripts_migrations_and_platform_files() {
        let index = ProjectIndex::new(PathBuf::from("."));
        let tracker = WorkingSetTracker::new();

        let report = RepoIntelligenceReport::for_changes(
            &index,
            &[
                PathBuf::from("build.rs"),
                PathBuf::from("db/migrations/001.sql"),
                PathBuf::from("zhongshu-orb/src/overlay_windows.rs"),
            ],
            "fix build",
            &tracker,
        );

        assert!(report
            .risks
            .iter()
            .any(|risk| risk.kind == RepoRiskKind::BuildScript));
        assert!(report
            .risks
            .iter()
            .any(|risk| risk.kind == RepoRiskKind::Migration));
        assert!(report
            .risks
            .iter()
            .any(|risk| risk.kind == RepoRiskKind::PlatformSpecific));
    }

    #[test]
    fn working_set_prioritizes_failures_and_edits() {
        let mut tracker = WorkingSetTracker::new();
        tracker.record(WorkingSetSignal::FileRead {
            path: PathBuf::from("src/a.rs"),
        });
        tracker.record(WorkingSetSignal::FileEdit {
            path: PathBuf::from("src/b.rs"),
        });
        tracker.record(WorkingSetSignal::TestFailure {
            path: PathBuf::from("src/b.rs"),
            command: "cargo test".into(),
        });

        let snapshot = tracker.snapshot();

        assert_eq!(snapshot.files[0].path, PathBuf::from("src/b.rs"));
        assert!(snapshot.files[0].score > snapshot.files[1].score);
        assert_eq!(snapshot.files[0].last_reason, "test failed: cargo test");
    }
}
