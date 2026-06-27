use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellSemantics {
    pub raw_command: String,
    pub normalized_command: String,
    pub segments: Vec<ShellSegment>,
    pub class: ShellCommandClass,
    pub is_verification: bool,
    pub is_background: bool,
    pub requires_approval: bool,
    pub block_reason: Option<String>,
}

impl ShellSemantics {
    pub fn analyze(raw_command: &str) -> Self {
        let normalized_command = raw_command.trim().to_string();
        let segments = split_segments(&normalized_command);
        let mut class = ShellCommandClass::Noop;
        let mut is_verification = false;
        let mut is_background = normalized_command.ends_with('&');
        let mut requires_approval = false;
        let mut block_reason = None;

        for segment in &segments {
            let segment_class = classify_segment(segment);
            class = class.combine(segment_class);
            is_verification |= segment_class == ShellCommandClass::Verification;
            is_background |= segment.operator_after == Some(ShellOperator::Background);

            if let Some(reason) = detect_block_reason(&segment.text) {
                block_reason = Some(reason);
            }
            if requires_segment_approval(segment, segment_class) {
                requires_approval = true;
            }
        }

        if block_reason.is_some() {
            class = ShellCommandClass::Blocked;
            requires_approval = true;
        }

        Self {
            raw_command: raw_command.to_string(),
            normalized_command,
            segments,
            class,
            is_verification,
            is_background,
            requires_approval,
            block_reason,
        }
    }

    pub fn from_tool_arguments(arguments: &serde_json::Value) -> Result<Self, ShellArgumentError> {
        let command = arguments
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or(ShellArgumentError::MissingCommand)?;
        Ok(Self::analyze(command))
    }

    pub fn from_tool_arguments_str(arguments: &str) -> Result<Self, ShellArgumentError> {
        let value: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| ShellArgumentError::InvalidJson {
                message: e.to_string(),
            })?;
        Self::from_tool_arguments(&value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellSegment {
    pub text: String,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub operator_before: Option<ShellOperator>,
    pub operator_after: Option<ShellOperator>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellOperator {
    And,
    Or,
    Pipe,
    Sequence,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ShellCommandClass {
    Noop,
    Read,
    Search,
    List,
    Build,
    Verification,
    Network,
    Mutation,
    Destructive,
    Unknown,
    Blocked,
}

impl ShellCommandClass {
    fn combine(self, other: Self) -> Self {
        use ShellCommandClass::*;
        match (self, other) {
            (Noop, other) => other,
            (this, Noop) => this,
            (Blocked, _) | (_, Blocked) => Blocked,
            (Destructive, _) | (_, Destructive) => Destructive,
            (Mutation, _) | (_, Mutation) => Mutation,
            (Network, _) | (_, Network) => Network,
            (Unknown, _) | (_, Unknown) => Unknown,
            (Verification, _) | (_, Verification) => Verification,
            (Build, _) | (_, Build) => Build,
            (Search, List) | (List, Search) => Search,
            (Search, _) | (_, Search) => Search,
            (List, _) | (_, List) => List,
            (Read, _) => Read,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellArgumentError {
    MissingCommand,
    InvalidJson { message: String },
}

impl std::fmt::Display for ShellArgumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShellArgumentError::MissingCommand => write!(f, "shell arguments are missing command"),
            ShellArgumentError::InvalidJson { message } => {
                write!(f, "invalid shell argument json: {message}")
            }
        }
    }
}

impl std::error::Error for ShellArgumentError {}

fn split_segments(command: &str) -> Vec<ShellSegment> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut operator_before = None;
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        if escaped {
            current.push(ch);
            escaped = false;
            i += 1;
            continue;
        }

        if ch == '\\' {
            current.push(ch);
            escaped = true;
            i += 1;
            continue;
        }

        if let Some(q) = quote {
            current.push(ch);
            if ch == q {
                quote = None;
            }
            i += 1;
            continue;
        }

        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            current.push(ch);
            i += 1;
            continue;
        }

        let op = match ch {
            '&' if chars.get(i + 1) == Some(&'&') => Some((ShellOperator::And, 2)),
            '|' if chars.get(i + 1) == Some(&'|') => Some((ShellOperator::Or, 2)),
            '|' => Some((ShellOperator::Pipe, 1)),
            ';' => Some((ShellOperator::Sequence, 1)),
            '&' => Some((ShellOperator::Background, 1)),
            _ => None,
        };

        if let Some((operator_after, width)) = op {
            push_segment(
                &mut segments,
                &mut current,
                operator_before,
                Some(operator_after),
            );
            operator_before = Some(operator_after);
            i += width;
            continue;
        }

        current.push(ch);
        i += 1;
    }

    push_segment(&mut segments, &mut current, operator_before, None);
    segments
}

fn push_segment(
    segments: &mut Vec<ShellSegment>,
    current: &mut String,
    operator_before: Option<ShellOperator>,
    operator_after: Option<ShellOperator>,
) {
    let text = current.trim().to_string();
    current.clear();
    if text.is_empty() {
        return;
    }
    let tokens = tokenize(&text);
    let program = tokens.first().cloned();
    let args = tokens.into_iter().skip(1).collect();
    segments.push(ShellSegment {
        text,
        program,
        args,
        operator_before,
        operator_after,
    });
}

fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for ch in text.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }

    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn classify_segment(segment: &ShellSegment) -> ShellCommandClass {
    let Some(program) = segment.program.as_deref().map(normalize_program) else {
        return ShellCommandClass::Noop;
    };
    let args = segment.args.join(" ").to_lowercase();

    if is_verification_command(&program, &segment.args) {
        return ShellCommandClass::Verification;
    }

    match program.as_str() {
        "ls" | "dir" | "find" | "fd" => ShellCommandClass::List,
        "cat" | "type" | "less" | "more" | "head" | "tail" | "sed" | "awk" | "wc" => {
            ShellCommandClass::Read
        }
        "grep" | "rg" | "ripgrep" | "where" | "where.exe" | "select-string" => {
            ShellCommandClass::Search
        }
        "cargo" if args.contains("build") || args.contains("check") => ShellCommandClass::Build,
        "npm" | "pnpm" | "yarn" if args.contains("build") => ShellCommandClass::Build,
        "go" if args.contains("build") => ShellCommandClass::Build,
        "curl" | "wget" | "git" | "gh" | "ssh" | "scp" | "rsync" => ShellCommandClass::Network,
        "rm" | "del" | "erase" | "move" | "mv" | "cp" | "copy" | "mkdir" | "rmdir" | "touch"
        | "chmod" | "chown" | "icacls" | "takeown" => ShellCommandClass::Mutation,
        "format" | "diskpart" | "mkfs" | "mkfs.ext4" | "mkfs.ntfs" | "reg" | "sc" | "taskkill"
        | "shutdown" => ShellCommandClass::Destructive,
        "powershell" | "pwsh" if args.contains("encodedcommand") => ShellCommandClass::Destructive,
        "python" | "python3" | "node" | "deno" | "bun" | "cmd" | "powershell" | "pwsh" => {
            ShellCommandClass::Unknown
        }
        _ => ShellCommandClass::Unknown,
    }
}

fn is_verification_command(program: &str, args: &[String]) -> bool {
    let joined = args.join(" ").to_lowercase();
    match program {
        "cargo" => {
            joined.contains("test")
                || joined.contains("check")
                || joined.contains("clippy")
                || joined.contains("fmt")
        }
        "npm" | "pnpm" | "yarn" | "bun" => joined.contains("test") || joined.contains("lint"),
        "go" => joined.contains("test"),
        "pytest" | "vitest" | "jest" | "mocha" | "ruff" | "mypy" => true,
        "python" | "python3" => joined.contains("pytest") || joined.contains("unittest"),
        _ => false,
    }
}

fn requires_segment_approval(segment: &ShellSegment, class: ShellCommandClass) -> bool {
    matches!(
        class,
        ShellCommandClass::Mutation
            | ShellCommandClass::Destructive
            | ShellCommandClass::Unknown
            | ShellCommandClass::Blocked
    ) || segment.operator_after == Some(ShellOperator::Background)
        || segment.operator_before == Some(ShellOperator::Sequence)
        || has_dangerous_expansion(&segment.text)
}

fn detect_block_reason(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    if lower.contains("rm -rf /")
        || lower.contains("del /s c:\\")
        || lower.contains("format c:")
        || lower.contains("diskpart")
        || lower.contains("reg delete hklm")
    {
        return Some("command targets system-destructive operation".into());
    }
    if lower.contains("curl ") && lower.contains("| sh") {
        return Some("network script is piped into shell".into());
    }
    None
}

fn has_dangerous_expansion(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("$(")
        || lower.contains('`')
        || lower.contains("<(")
        || lower.contains(">(")
        || lower.contains("${")
        || lower.contains("-encodedcommand")
}

fn normalize_program(program: &str) -> String {
    program
        .trim_matches('"')
        .trim_matches('\'')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_json_command_not_raw_json() {
        let semantics =
            ShellSemantics::from_tool_arguments_str(r#"{"command":"cargo test -p zhongshu-core"}"#)
                .unwrap();

        assert_eq!(semantics.class, ShellCommandClass::Verification);
        assert!(semantics.is_verification);
        assert!(!semantics.requires_approval);
    }

    #[test]
    fn splits_compound_commands_with_operators() {
        let semantics = ShellSemantics::analyze("rg foo src && cargo test -p zhongshu-core");

        assert_eq!(semantics.segments.len(), 2);
        assert_eq!(
            semantics.segments[0].operator_after,
            Some(ShellOperator::And)
        );
        assert_eq!(
            semantics.segments[1].operator_before,
            Some(ShellOperator::And)
        );
        assert_eq!(semantics.class, ShellCommandClass::Verification);
    }

    #[test]
    fn detects_background_commands() {
        let semantics = ShellSemantics::analyze("npm run dev &");

        assert!(semantics.is_background);
        assert!(semantics.requires_approval);
    }

    #[test]
    fn blocks_destructive_windows_command() {
        let semantics = ShellSemantics::analyze("reg delete HKLM\\Software\\Demo /f");

        assert_eq!(semantics.class, ShellCommandClass::Blocked);
        assert!(semantics.requires_approval);
        assert!(semantics.block_reason.is_some());
    }

    #[test]
    fn detects_command_substitution_as_approval_required() {
        let semantics = ShellSemantics::analyze("echo $(cat secret.txt)");

        assert!(semantics.requires_approval);
    }

    #[test]
    fn keeps_quoted_operators_inside_segment() {
        let semantics = ShellSemantics::analyze("rg \"foo && bar\" src");

        assert_eq!(semantics.segments.len(), 1);
        assert_eq!(semantics.class, ShellCommandClass::Search);
    }
}
