use std::env;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn main() {
    if let Err(err) = run() {
        eprintln!("xtask: {err}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("proof") => run_proof(&args[1..]),
        Some("-h") | Some("--help") | None => {
            print_help();
            Ok(())
        }
        Some(other) => Err(format!("unknown command '{other}'")),
    }
}

fn print_help() {
    println!(
        "Usage:\n  cargo xtask proof --mode local\n\nModes:\n  local | pr | baseline | release"
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProofMode {
    Local,
    Pr,
    Baseline,
    Release,
}

impl ProofMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "local" => Ok(Self::Local),
            "pr" => Ok(Self::Pr),
            "baseline" => Ok(Self::Baseline),
            "release" => Ok(Self::Release),
            other => Err(format!("unknown proof mode '{other}'")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Pr => "pr",
            Self::Baseline => "baseline",
            Self::Release => "release",
        }
    }
}

#[derive(Debug, Clone)]
struct ProofArgs {
    mode: ProofMode,
}

fn parse_proof_args(args: &[String]) -> Result<ProofArgs, String> {
    let mut mode = ProofMode::Local;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--mode" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("--mode requires a value".into());
                };
                mode = ProofMode::parse(value)?;
                index += 2;
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown proof argument '{other}'")),
        }
    }
    Ok(ProofArgs { mode })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckStatus {
    Passed,
    Failed,
    Skipped,
}

impl CheckStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone)]
struct CheckSpec {
    id: &'static str,
    title: &'static str,
    command: Vec<&'static str>,
    skip_reason: Option<&'static str>,
}

#[derive(Debug, Clone)]
struct CheckResult {
    id: String,
    title: String,
    status: CheckStatus,
    command: Vec<String>,
    duration_ms: Option<u128>,
    exit_code: Option<i32>,
    log_path: Option<String>,
    skip_reason: Option<String>,
}

fn run_proof(args: &[String]) -> Result<(), String> {
    let args = parse_proof_args(args)?;
    let workspace = workspace_root()?;
    let generated_at = unix_secs();
    let run_dir = workspace.join("artifacts").join("proof-runs").join(format!(
        "{}-{}",
        generated_at,
        args.mode.as_str()
    ));
    fs::create_dir_all(run_dir.join("logs"))
        .map_err(|err| format!("cannot create {}: {err}", run_dir.display()))?;

    let specs = local_check_specs();
    let mut results = Vec::with_capacity(specs.len());
    for spec in specs {
        let result = run_check(&workspace, &run_dir, &spec)?;
        results.push(result);
    }

    write_report_json(
        &run_dir.join("report.json"),
        args.mode,
        generated_at,
        &results,
    )?;
    write_report_markdown(
        &run_dir.join("report.md"),
        args.mode,
        generated_at,
        &results,
    )?;

    let summary = summarize(&results);
    println!(
        "proof report: {}\npassed={} failed={} skipped={}",
        run_dir.display(),
        summary.passed,
        summary.failed,
        summary.skipped
    );

    if summary.failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn workspace_root() -> Result<PathBuf, String> {
    env::current_dir().map_err(|err| format!("cannot read current dir: {err}"))
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs()
}

fn local_check_specs() -> Vec<CheckSpec> {
    vec![
        CheckSpec {
            id: "cargo-fmt",
            title: "Rust formatting",
            command: vec!["cargo", "fmt", "--check"],
            skip_reason: None,
        },
        CheckSpec {
            id: "core-tests",
            title: "Core tests",
            command: vec!["cargo", "test", "-p", "zhongshu-core"],
            skip_reason: None,
        },
        CheckSpec {
            id: "orb-check",
            title: "Orb compile check",
            command: vec!["cargo", "check", "-p", "zhongshu-orb"],
            skip_reason: None,
        },
        CheckSpec {
            id: "orb-tests",
            title: "Orb tests",
            command: vec!["cargo", "test", "-p", "zhongshu-orb"],
            skip_reason: None,
        },
        CheckSpec {
            id: "ui-typecheck",
            title: "UI TypeScript typecheck",
            command: vec!["pnpm", "--dir", "zhongshu-orb/ui", "typecheck"],
            skip_reason: None,
        },
        CheckSpec {
            id: "ui-tests",
            title: "UI tests",
            command: vec!["pnpm", "--dir", "zhongshu-orb/ui", "test"],
            skip_reason: None,
        },
        CheckSpec {
            id: "ui-build",
            title: "UI production build",
            command: vec!["pnpm", "--dir", "zhongshu-orb/ui", "build"],
            skip_reason: None,
        },
        CheckSpec {
            id: "git-diff-check",
            title: "Git whitespace check",
            command: vec!["git", "diff", "--check"],
            skip_reason: None,
        },
        CheckSpec {
            id: "windows-webview2-visual",
            title: "Windows WebView2 visual smoke",
            command: Vec::new(),
            skip_reason: Some(
                "manual desktop evidence required: screenshot/log for WebView2 window startup, focus, resize, and close-hide behavior",
            ),
        },
        CheckSpec {
            id: "ubuntu-gtk-visual",
            title: "Ubuntu GTK visual smoke",
            command: Vec::new(),
            skip_reason: Some(
                "manual desktop evidence required when GTK overlay changes; user previously reported Ubuntu command execution path works",
            ),
        },
    ]
}

fn run_check(workspace: &Path, run_dir: &Path, spec: &CheckSpec) -> Result<CheckResult, String> {
    if let Some(reason) = spec.skip_reason {
        println!("skip {}: {}", spec.id, reason);
        return Ok(CheckResult {
            id: spec.id.into(),
            title: spec.title.into(),
            status: CheckStatus::Skipped,
            command: spec.command.iter().map(|part| (*part).into()).collect(),
            duration_ms: None,
            exit_code: None,
            log_path: None,
            skip_reason: Some(reason.into()),
        });
    }

    let Some((program, command_args)) = spec.command.split_first() else {
        return Err(format!("check '{}' has no command", spec.id));
    };

    println!("run {}: {}", spec.id, spec.command.join(" "));
    let started = Instant::now();
    let output = run_command(workspace, program, command_args, &spec.command)?;
    let duration_ms = started.elapsed().as_millis();
    let status = if output.status.success() {
        CheckStatus::Passed
    } else {
        CheckStatus::Failed
    };
    let log_path = run_dir.join("logs").join(format!("{}.log", spec.id));
    write_check_log(&log_path, spec, &output, duration_ms)?;
    let relative_log_path =
        path_slash(log_path.strip_prefix(run_dir).unwrap_or(log_path.as_path()));

    Ok(CheckResult {
        id: spec.id.into(),
        title: spec.title.into(),
        status,
        command: spec.command.iter().map(|part| (*part).into()).collect(),
        duration_ms: Some(duration_ms),
        exit_code: output.status.code(),
        log_path: Some(relative_log_path),
        skip_reason: None,
    })
}

fn run_command(
    workspace: &Path,
    program: &str,
    command_args: &[&str],
    display_command: &[&str],
) -> Result<Output, String> {
    match Command::new(program)
        .args(command_args.iter().map(OsStr::new))
        .current_dir(workspace)
        .output()
    {
        Ok(output) => Ok(output),
        Err(err) if cfg!(windows) && err.kind() == io::ErrorKind::NotFound => Command::new("cmd")
            .arg("/C")
            .arg(program)
            .args(command_args.iter().map(OsStr::new))
            .current_dir(workspace)
            .output()
            .map_err(|fallback_err| {
                format!(
                    "cannot run '{}' directly ({err}) or through cmd.exe ({fallback_err})",
                    display_command.join(" ")
                )
            }),
        Err(err) => Err(format!("cannot run '{}': {err}", display_command.join(" "))),
    }
}

fn write_check_log(
    path: &Path,
    spec: &CheckSpec,
    output: &std::process::Output,
    duration_ms: u128,
) -> Result<(), String> {
    let mut text = String::new();
    writeln!(&mut text, "$ {}", spec.command.join(" ")).unwrap();
    writeln!(&mut text, "status: {}", output.status).unwrap();
    writeln!(&mut text, "duration_ms: {duration_ms}").unwrap();
    writeln!(&mut text, "\n--- stdout ---").unwrap();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    writeln!(&mut text, "\n--- stderr ---").unwrap();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    fs::write(path, text).map_err(|err| format!("cannot write {}: {err}", path.display()))
}

#[derive(Default)]
struct Summary {
    passed: usize,
    failed: usize,
    skipped: usize,
}

fn summarize(results: &[CheckResult]) -> Summary {
    let mut summary = Summary::default();
    for result in results {
        match result.status {
            CheckStatus::Passed => summary.passed += 1,
            CheckStatus::Failed => summary.failed += 1,
            CheckStatus::Skipped => summary.skipped += 1,
        }
    }
    summary
}

fn write_report_json(
    path: &Path,
    mode: ProofMode,
    generated_at: u64,
    results: &[CheckResult],
) -> Result<(), String> {
    let summary = summarize(results);
    let mut json = String::new();
    writeln!(&mut json, "{{").unwrap();
    writeln!(&mut json, "  \"schema_version\": 1,").unwrap();
    writeln!(&mut json, "  \"mode\": \"{}\",", mode.as_str()).unwrap();
    writeln!(&mut json, "  \"generated_at_unix_secs\": {generated_at},").unwrap();
    writeln!(
        &mut json,
        "  \"summary\": {{ \"passed\": {}, \"failed\": {}, \"skipped\": {} }},",
        summary.passed, summary.failed, summary.skipped
    )
    .unwrap();
    writeln!(&mut json, "  \"checks\": [").unwrap();
    for (index, result) in results.iter().enumerate() {
        let comma = if index + 1 == results.len() { "" } else { "," };
        writeln!(&mut json, "    {{").unwrap();
        writeln!(&mut json, "      \"id\": \"{}\",", json_escape(&result.id)).unwrap();
        writeln!(
            &mut json,
            "      \"title\": \"{}\",",
            json_escape(&result.title)
        )
        .unwrap();
        writeln!(
            &mut json,
            "      \"status\": \"{}\",",
            result.status.as_str()
        )
        .unwrap();
        writeln!(
            &mut json,
            "      \"command\": {},",
            json_string_array(&result.command)
        )
        .unwrap();
        write_optional_u128(&mut json, "duration_ms", result.duration_ms, true);
        write_optional_i32(&mut json, "exit_code", result.exit_code, true);
        write_optional_string(&mut json, "log_path", result.log_path.as_deref(), true);
        write_optional_string(
            &mut json,
            "skip_reason",
            result.skip_reason.as_deref(),
            false,
        );
        writeln!(&mut json, "    }}{comma}").unwrap();
    }
    writeln!(&mut json, "  ]").unwrap();
    writeln!(&mut json, "}}").unwrap();
    fs::write(path, json).map_err(|err| format!("cannot write {}: {err}", path.display()))
}

fn write_report_markdown(
    path: &Path,
    mode: ProofMode,
    generated_at: u64,
    results: &[CheckResult],
) -> Result<(), String> {
    let summary = summarize(results);
    let mut markdown = String::new();
    writeln!(&mut markdown, "# Zhongshu Proof Report").unwrap();
    writeln!(&mut markdown).unwrap();
    writeln!(&mut markdown, "- Mode: `{}`", mode.as_str()).unwrap();
    writeln!(&mut markdown, "- Generated: `{generated_at}`").unwrap();
    writeln!(&mut markdown, "- Passed: `{}`", summary.passed).unwrap();
    writeln!(&mut markdown, "- Failed: `{}`", summary.failed).unwrap();
    writeln!(&mut markdown, "- Skipped: `{}`", summary.skipped).unwrap();
    writeln!(&mut markdown).unwrap();
    writeln!(&mut markdown, "| Check | Status | Command | Log / Reason |").unwrap();
    writeln!(&mut markdown, "| --- | --- | --- | --- |").unwrap();
    for result in results {
        let command = if result.command.is_empty() {
            String::new()
        } else {
            format!("`{}`", result.command.join(" "))
        };
        let detail = result
            .log_path
            .as_ref()
            .map(|path| format!("[log]({path})"))
            .or_else(|| result.skip_reason.clone())
            .unwrap_or_default();
        writeln!(
            &mut markdown,
            "| {} | `{}` | {} | {} |",
            result.title,
            result.status.as_str(),
            command,
            detail
        )
        .unwrap();
    }
    fs::write(path, markdown).map_err(|err| format!("cannot write {}: {err}", path.display()))
}

fn write_optional_u128(out: &mut String, key: &str, value: Option<u128>, comma: bool) {
    let suffix = if comma { "," } else { "" };
    match value {
        Some(value) => writeln!(out, "      \"{key}\": {value}{suffix}").unwrap(),
        None => writeln!(out, "      \"{key}\": null{suffix}").unwrap(),
    }
}

fn write_optional_i32(out: &mut String, key: &str, value: Option<i32>, comma: bool) {
    let suffix = if comma { "," } else { "" };
    match value {
        Some(value) => writeln!(out, "      \"{key}\": {value}{suffix}").unwrap(),
        None => writeln!(out, "      \"{key}\": null{suffix}").unwrap(),
    }
}

fn write_optional_string(out: &mut String, key: &str, value: Option<&str>, comma: bool) {
    let suffix = if comma { "," } else { "" };
    match value {
        Some(value) => {
            writeln!(out, "      \"{key}\": \"{}\"{suffix}", json_escape(value)).unwrap()
        }
        None => writeln!(out, "      \"{key}\": null{suffix}").unwrap(),
    }
}

fn json_string_array(values: &[String]) -> String {
    let mut out = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push('"');
        out.push_str(&json_escape(value));
        out.push('"');
    }
    out.push(']');
    out
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => {
                write!(&mut escaped, "\\u{:04x}", ch as u32).unwrap();
            }
            ch => escaped.push(ch),
        }
    }
    escaped
}

fn path_slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_and_explicit_modes() {
        let default_args = parse_proof_args(&[]).unwrap();
        assert_eq!(default_args.mode, ProofMode::Local);

        let explicit_args = parse_proof_args(&["--mode".into(), "baseline".into()]).unwrap();
        assert_eq!(explicit_args.mode, ProofMode::Baseline);
    }

    #[test]
    fn rejects_unknown_mode() {
        let err = parse_proof_args(&["--mode".into(), "unknown".into()]).unwrap_err();
        assert!(err.contains("unknown proof mode"));
    }

    #[test]
    fn summary_counts_statuses() {
        let results = vec![
            check_result("fmt", CheckStatus::Passed),
            check_result("test", CheckStatus::Failed),
            check_result("ui", CheckStatus::Skipped),
        ];

        let summary = summarize(&results);

        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.skipped, 1);
    }

    #[test]
    fn json_escape_handles_quotes_slashes_and_control_chars() {
        assert_eq!(json_escape("a\"b\\c\n"), "a\\\"b\\\\c\\n");
    }

    fn check_result(id: &str, status: CheckStatus) -> CheckResult {
        CheckResult {
            id: id.into(),
            title: id.into(),
            status,
            command: Vec::new(),
            duration_ms: None,
            exit_code: None,
            log_path: None,
            skip_reason: None,
        }
    }
}
