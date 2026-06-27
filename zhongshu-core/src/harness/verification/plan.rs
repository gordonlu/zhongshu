use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::tool::shell_semantics::{ShellCommandClass, ShellSemantics};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationPlan {
    pub required: bool,
    pub commands: Vec<VerificationCommand>,
    pub environment_notes: Vec<String>,
    pub fallback_commands: Vec<VerificationCommand>,
}

impl VerificationPlan {
    pub fn empty() -> Self {
        Self {
            required: false,
            commands: Vec::new(),
            environment_notes: Vec::new(),
            fallback_commands: Vec::new(),
        }
    }

    pub fn for_changes(changed_files: &[PathBuf], task_description: &str) -> Self {
        let required = infer_required_from_changes(changed_files, task_description);
        if !required {
            return Self::empty();
        }

        let mut commands = Vec::new();
        let mut fallback_commands = Vec::new();
        let mut environment_notes = Vec::new();

        if touches_rust(changed_files) {
            commands.push(VerificationCommand::new(
                "cargo test -p zhongshu-core",
                VerificationReason::RustCoreChange,
            ));
            fallback_commands.push(VerificationCommand::new(
                "cargo check -p zhongshu-core",
                VerificationReason::RustCoreChange,
            ));
        }

        if touches_orb(changed_files) {
            commands.push(VerificationCommand::new(
                "cargo check -p zhongshu-orb",
                VerificationReason::DesktopIntegrationChange,
            ));
            environment_notes.push(
                "zhongshu-orb may require platform UI dependencies; use non-UI crate checks when unavailable"
                    .into(),
            );
        }

        if touches_frontend_asset(changed_files) {
            commands.push(VerificationCommand::new(
                "cargo check -p zhongshu-orb",
                VerificationReason::FrontendAssetChange,
            ));
        }

        if commands.is_empty() {
            commands.push(VerificationCommand::new(
                "cargo test -p zhongshu-core",
                VerificationReason::GenericCodeChange,
            ));
        }

        dedup_commands(&mut commands);
        dedup_commands(&mut fallback_commands);

        Self {
            required,
            commands,
            environment_notes,
            fallback_commands,
        }
    }

    pub fn from_shell_commands(commands: &[String]) -> Self {
        let mut verification_commands = Vec::new();
        for command in commands {
            let semantics = ShellSemantics::analyze(command);
            if semantics.is_verification {
                verification_commands.push(VerificationCommand {
                    command: command.clone(),
                    reason: VerificationReason::UserProvided,
                    class: semantics.class,
                });
            }
        }
        Self {
            required: !verification_commands.is_empty(),
            commands: verification_commands,
            environment_notes: Vec::new(),
            fallback_commands: Vec::new(),
        }
    }

    pub fn merge(mut self, mut other: Self) -> Self {
        self.required |= other.required;
        self.commands.append(&mut other.commands);
        self.environment_notes.append(&mut other.environment_notes);
        self.fallback_commands.append(&mut other.fallback_commands);
        dedup_commands(&mut self.commands);
        dedup_commands(&mut self.fallback_commands);
        self.environment_notes.sort();
        self.environment_notes.dedup();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationCommand {
    pub command: String,
    pub reason: VerificationReason,
    pub class: ShellCommandClass,
}

impl VerificationCommand {
    pub fn new(command: impl Into<String>, reason: VerificationReason) -> Self {
        let command = command.into();
        let semantics = ShellSemantics::analyze(&command);
        Self {
            command,
            reason,
            class: semantics.class,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationReason {
    RustCoreChange,
    DesktopIntegrationChange,
    FrontendAssetChange,
    GenericCodeChange,
    UserProvided,
}

pub fn infer_required(task_description: &str) -> bool {
    infer_required_from_changes(&[], task_description)
}

pub fn infer_required_from_changes(changed_files: &[PathBuf], task_description: &str) -> bool {
    let task = task_description.to_lowercase();
    task.contains("fix")
        || task.contains("implement")
        || task.contains("refactor")
        || task.contains("test")
        || task.contains("verify")
        || task.contains("修改")
        || task.contains("修复")
        || task.contains("实现")
        || changed_files.iter().any(is_code_or_config)
}

fn touches_rust(files: &[PathBuf]) -> bool {
    files.iter().any(|path| {
        path.extension().and_then(|s| s.to_str()) == Some("rs")
            && path.components().any(|c| c.as_os_str() == "zhongshu-core")
    })
}

fn touches_orb(files: &[PathBuf]) -> bool {
    files
        .iter()
        .any(|path| path.components().any(|c| c.as_os_str() == "zhongshu-orb"))
}

fn touches_frontend_asset(files: &[PathBuf]) -> bool {
    files.iter().any(|path| {
        path.components().any(|c| c.as_os_str() == "assets")
            && matches!(
                path.extension().and_then(|s| s.to_str()),
                Some("html" | "css" | "js")
            )
    })
}

fn is_code_or_config(path: &PathBuf) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some(
            "rs" | "toml"
                | "lock"
                | "html"
                | "css"
                | "js"
                | "ts"
                | "tsx"
                | "jsx"
                | "py"
                | "go"
                | "java"
                | "cs"
                | "json"
                | "yaml"
                | "yml"
        )
    )
}

fn dedup_commands(commands: &mut Vec<VerificationCommand>) {
    commands.sort_by(|a, b| a.command.cmp(&b.command));
    commands.dedup_by(|a, b| a.command == b.command);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_text_can_require_verification() {
        assert!(infer_required("fix shell parsing"));
        assert!(infer_required("实现新功能"));
        assert!(!infer_required("explain architecture"));
    }

    #[test]
    fn rust_core_change_gets_core_test_plan() {
        let plan = VerificationPlan::for_changes(
            &[PathBuf::from("zhongshu-core/src/tool/mod.rs")],
            "implement feature",
        );

        assert!(plan.required);
        assert!(plan
            .commands
            .iter()
            .any(|cmd| cmd.command == "cargo test -p zhongshu-core"));
        assert!(plan
            .fallback_commands
            .iter()
            .any(|cmd| cmd.command == "cargo check -p zhongshu-core"));
    }

    #[test]
    fn orb_change_gets_orb_check_and_environment_note() {
        let plan = VerificationPlan::for_changes(
            &[PathBuf::from("zhongshu-orb/src/app.rs")],
            "fix desktop",
        );

        assert!(plan
            .commands
            .iter()
            .any(|cmd| cmd.command == "cargo check -p zhongshu-orb"));
        assert!(!plan.environment_notes.is_empty());
    }

    #[test]
    fn shell_commands_filter_to_verification_only() {
        let plan = VerificationPlan::from_shell_commands(&[
            "rg TODO zhongshu-core".into(),
            "cargo test -p zhongshu-core".into(),
        ]);

        assert_eq!(plan.commands.len(), 1);
        assert_eq!(plan.commands[0].class, ShellCommandClass::Verification);
    }

    #[test]
    fn merge_dedups_commands_and_notes() {
        let left =
            VerificationPlan::for_changes(&[PathBuf::from("zhongshu-core/src/lib.rs")], "fix");
        let right = VerificationPlan::from_shell_commands(&["cargo test -p zhongshu-core".into()]);

        let merged = left.merge(right);

        assert_eq!(
            merged
                .commands
                .iter()
                .filter(|cmd| cmd.command == "cargo test -p zhongshu-core")
                .count(),
            1
        );
    }
}
