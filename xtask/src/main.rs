use std::collections::BTreeSet;
use std::env;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::net::TcpListener;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ProofArea {
    CoreRuntime,
    Harness,
    StorageRuntime,
    EquipmentMcp,
    DesktopUi,
    ProofRunner,
    DocsRoadmap,
}

impl ProofArea {
    fn as_str(self) -> &'static str {
        match self {
            Self::CoreRuntime => "core-runtime",
            Self::Harness => "harness",
            Self::StorageRuntime => "storage-runtime",
            Self::EquipmentMcp => "equipment-mcp",
            Self::DesktopUi => "desktop-ui",
            Self::ProofRunner => "proof-runner",
            Self::DocsRoadmap => "docs-roadmap",
        }
    }
}

impl ProofMode {
    fn all() -> &'static [ProofMode] {
        &[Self::Local, Self::Pr, Self::Baseline, Self::Release]
    }

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

    fn policy(self) -> &'static str {
        match self {
            Self::Local => "fast routed checks for changed areas",
            Self::Pr => "routed checks plus full attack matrix",
            Self::Baseline => "PR checks plus workspace build regression",
            Self::Release => {
                "full proof including all tests, workspace build, and required desktop evidence"
            }
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
    areas: &'static [ProofArea],
    modes: &'static [ProofMode],
    requires_loopback_bind: bool,
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

#[derive(Debug, Clone)]
struct ProofSelection {
    changed_files: Vec<String>,
    changed_areas: BTreeSet<ProofArea>,
    loopback_bind_available: bool,
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

    let selection = build_selection(&workspace)?;
    let specs = proof_check_specs();
    let mut results = Vec::with_capacity(specs.len());
    for spec in specs {
        let result = run_check(&workspace, &run_dir, &spec, args.mode, &selection)?;
        results.push(result);
    }

    write_report_json(
        &run_dir.join("report.json"),
        args.mode,
        generated_at,
        &selection,
        &results,
    )?;
    write_report_markdown(
        &run_dir.join("report.md"),
        args.mode,
        generated_at,
        &selection,
        &results,
    )?;
    write_junit_xml(&run_dir.join("junit.xml"), &results)?;

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

fn build_selection(workspace: &Path) -> Result<ProofSelection, String> {
    let changed_files = changed_files(workspace)?;
    let changed_areas = classify_changed_areas(&changed_files);
    let loopback_bind_available = TcpListener::bind("127.0.0.1:0").is_ok();
    Ok(ProofSelection {
        changed_files,
        changed_areas,
        loopback_bind_available,
    })
}

fn changed_files(workspace: &Path) -> Result<Vec<String>, String> {
    let output = Command::new("git")
        .args(["diff", "--name-only", "HEAD"])
        .current_dir(workspace)
        .output()
        .map_err(|err| format!("cannot inspect changed files: {err}"))?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(ToString::to_string)
        .collect())
}

fn classify_changed_areas(files: &[String]) -> BTreeSet<ProofArea> {
    let mut areas = BTreeSet::new();
    for file in files {
        if file.starts_with("zhongshu-orb/") {
            areas.insert(ProofArea::DesktopUi);
        } else if file.starts_with("xtask/") || file == ".roadmap/TEST_ROADMAP.md" {
            areas.insert(ProofArea::ProofRunner);
        } else if file.starts_with(".roadmap/") || file.ends_with(".md") {
            areas.insert(ProofArea::DocsRoadmap);
        } else if file.starts_with("zhongshu-core/src/harness/")
            || file.starts_with("zhongshu-core/tests/harness_attack")
        {
            areas.insert(ProofArea::Harness);
            if file.starts_with("zhongshu-core/tests/harness_attack_equipment") {
                areas.insert(ProofArea::EquipmentMcp);
            }
        } else if file.starts_with("zhongshu-core/src/equipment/") {
            areas.insert(ProofArea::EquipmentMcp);
        } else if file.starts_with("zhongshu-core/src/integration/")
            || file.starts_with("zhongshu-core/src/core/")
        {
            areas.insert(ProofArea::StorageRuntime);
        } else if file.starts_with("zhongshu-core/src/") {
            areas.insert(ProofArea::CoreRuntime);
        }
    }
    areas
}

fn proof_check_specs() -> Vec<CheckSpec> {
    vec![
        CheckSpec {
            id: "cargo-fmt",
            title: "Rust formatting",
            command: vec!["cargo", "fmt", "--check"],
            areas: &[],
            modes: &[ProofMode::Local, ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "core-tests",
            title: "Core tests",
            command: vec![
                "cargo",
                "test",
                "-p",
                "zhongshu-core",
                "--lib",
                "--",
                "--skip",
                "integration::deeplossless::tests::file_claims_roundtrip_through_lcm_endpoint",
                "--skip",
                "integration::deeplossless::tests::lcm_url_formats_endpoint_with_separator",
                "--skip",
                "integration::deeplossless::tests::proxy_rejects_without_api_key",
                "--skip",
                "integration::deeplossless::tests::proxy_starts_and_listens",
            ],
            areas: &[
                ProofArea::CoreRuntime,
                ProofArea::Harness,
                ProofArea::StorageRuntime,
                ProofArea::EquipmentMcp,
                ProofArea::ProofRunner,
            ],
            modes: &[ProofMode::Local, ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "deeplossless-proxy-tests",
            title: "Deeplossless proxy loopback tests",
            command: vec![
                "cargo",
                "test",
                "-p",
                "zhongshu-core",
                "integration::deeplossless::tests::",
            ],
            areas: &[ProofArea::StorageRuntime, ProofArea::ProofRunner],
            modes: &[ProofMode::Local, ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: true,
            skip_reason: None,
        },
        CheckSpec {
            id: "orb-check",
            title: "Orb compile check",
            command: vec!["cargo", "check", "-p", "zhongshu-orb"],
            areas: &[ProofArea::DesktopUi, ProofArea::ProofRunner],
            modes: &[ProofMode::Local, ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "orb-tests",
            title: "Orb tests",
            command: vec!["cargo", "test", "-p", "zhongshu-orb", "--bin", "zhongshu-orb"],
            areas: &[ProofArea::DesktopUi, ProofArea::ProofRunner],
            modes: &[ProofMode::Local, ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "ui-typecheck",
            title: "UI TypeScript typecheck",
            command: vec![
                "node",
                "zhongshu-orb/ui/node_modules/typescript/bin/tsc",
                "-b",
                "zhongshu-orb/ui/tsconfig.json",
            ],
            areas: &[ProofArea::DesktopUi, ProofArea::ProofRunner],
            modes: &[ProofMode::Local, ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "ui-tests",
            title: "UI tests",
            command: vec![
                "node",
                "zhongshu-orb/ui/node_modules/vitest/vitest.mjs",
                "run",
                "--root",
                "zhongshu-orb/ui",
            ],
            areas: &[ProofArea::DesktopUi, ProofArea::ProofRunner],
            modes: &[ProofMode::Local, ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "ui-build",
            title: "UI production build",
            command: vec![
                "node",
                "zhongshu-orb/ui/node_modules/vite/bin/vite.js",
                "build",
                "zhongshu-orb/ui",
                "--config",
                "zhongshu-orb/ui/vite.config.ts",
            ],
            areas: &[ProofArea::DesktopUi, ProofArea::ProofRunner],
            modes: &[ProofMode::Local, ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "capability-replay-fixtures",
            title: "Capability replay fixtures",
            command: vec![
                "cargo",
                "test",
                "-p",
                "zhongshu-core",
                "harness::verification::proof::tests::first_wave_replay_fixtures_pass_expected_assertions",
                "harness::verification::proof::tests::fixture_files_are_valid_json",
                "harness::verification::proof::tests::fixture_files_deserialize_with_schema_v1",
                "harness::verification::proof::tests::fixture_files_roundtrip_with_expected_assertions",
            ],
            areas: &[ProofArea::CoreRuntime, ProofArea::Harness, ProofArea::ProofRunner],
            modes: &[ProofMode::Local, ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "capability-replay-converter",
            title: "Capability replay deeplossless converter",
            command: vec![
                "cargo",
                "test",
                "-p",
                "zhongshu-core",
                "harness::verification::proof::tests::replay_to_harness_converts_tool_call_start",
                "harness::verification::proof::tests::replay_to_harness_detects_error_outcome",
                "harness::verification::proof::tests::replay_to_harness_detects_verification_failure",
                "harness::verification::proof::tests::replay_to_harness_empty_events_falls_back",
                "harness::verification::proof::tests::replay_to_harness_injects_final_run_completed",
            ],
            areas: &[ProofArea::CoreRuntime, ProofArea::Harness, ProofArea::ProofRunner],
            modes: &[ProofMode::Local, ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "capability-replay-source-loader",
            title: "Capability replay source loader",
            command: vec![
                "cargo",
                "test",
                "-p",
                "zhongshu-core",
                "harness::verification::proof::tests::replay_source_file_loads_evidence",
                "harness::verification::proof::tests::replay_source_missing_case_returns_none",
                "harness::verification::proof::tests::replay_source_deeplossless_returns_none_when_unavailable",
                "--test-threads=1",
            ],
            areas: &[ProofArea::CoreRuntime, ProofArea::Harness, ProofArea::ProofRunner],
            modes: &[ProofMode::Local, ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "capability-metadata",
            title: "Capability case metadata",
            command: vec![
                "cargo",
                "test",
                "-p",
                "zhongshu-core",
                "harness::verification::proof::tests::capability_cases_cover_first_wave",
            ],
            areas: &[ProofArea::CoreRuntime, ProofArea::Harness, ProofArea::ProofRunner],
            modes: &[ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "harness-attack",
            title: "Harness core attack matrix",
            command: vec!["cargo", "test", "-p", "zhongshu-core", "--test", "harness_attack"],
            areas: &[ProofArea::CoreRuntime, ProofArea::Harness, ProofArea::ProofRunner],
            modes: &[ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "harness-attack-shell",
            title: "Harness shell attack matrix",
            command: vec![
                "cargo",
                "test",
                "-p",
                "zhongshu-core",
                "--test",
                "harness_attack_shell",
            ],
            areas: &[ProofArea::CoreRuntime, ProofArea::Harness, ProofArea::ProofRunner],
            modes: &[ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "harness-attack-filesystem",
            title: "Harness filesystem attack matrix",
            command: vec![
                "cargo",
                "test",
                "-p",
                "zhongshu-core",
                "--test",
                "harness_attack_filesystem",
            ],
            areas: &[ProofArea::CoreRuntime, ProofArea::Harness, ProofArea::ProofRunner],
            modes: &[ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "harness-attack-edit",
            title: "Harness edit attack matrix",
            command: vec![
                "cargo",
                "test",
                "-p",
                "zhongshu-core",
                "--test",
                "harness_attack_edit",
            ],
            areas: &[ProofArea::CoreRuntime, ProofArea::Harness, ProofArea::ProofRunner],
            modes: &[ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "harness-attack-verify",
            title: "Harness verification attack matrix",
            command: vec![
                "cargo",
                "test",
                "-p",
                "zhongshu-core",
                "--test",
                "harness_attack_verify",
            ],
            areas: &[ProofArea::CoreRuntime, ProofArea::Harness, ProofArea::ProofRunner],
            modes: &[ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "harness-attack-browser",
            title: "Harness browser automation attack matrix",
            command: vec![
                "cargo",
                "test",
                "-p",
                "zhongshu-core",
                "--test",
                "harness_attack_browser",
            ],
            areas: &[ProofArea::CoreRuntime, ProofArea::Harness, ProofArea::ProofRunner],
            modes: &[ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "harness-attack-equipment",
            title: "Harness equipment and MCP attack matrix",
            command: vec![
                "cargo",
                "test",
                "-p",
                "zhongshu-core",
                "--test",
                "harness_attack_equipment",
            ],
            areas: &[
                ProofArea::CoreRuntime,
                ProofArea::Harness,
                ProofArea::EquipmentMcp,
                ProofArea::ProofRunner,
            ],
            modes: &[ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "workspace-build",
            title: "Workspace build (baseline+ regression)",
            command: vec!["cargo", "build", "--workspace"],
            areas: &[],
            modes: &[ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "workspace-all-tests",
            title: "Workspace all tests (release regression)",
            command: vec!["cargo", "test", "--workspace"],
            areas: &[],
            modes: &[ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "git-diff-check",
            title: "Git whitespace check",
            command: vec!["git", "diff", "--check"],
            areas: &[],
            modes: &[ProofMode::Local, ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: None,
        },
        CheckSpec {
            id: "windows-webview2-visual",
            title: "Windows WebView2 visual smoke",
            command: Vec::new(),
            areas: &[ProofArea::DesktopUi],
            modes: &[ProofMode::Local, ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: Some(
                "manual desktop evidence required: screenshot/log for WebView2 window startup, focus, resize, and close-hide behavior",
            ),
        },
        CheckSpec {
            id: "ubuntu-gtk-visual",
            title: "Ubuntu GTK visual smoke",
            command: Vec::new(),
            areas: &[ProofArea::DesktopUi],
            modes: &[ProofMode::Local, ProofMode::Pr, ProofMode::Baseline, ProofMode::Release],
            requires_loopback_bind: false,
            skip_reason: Some(
                "manual desktop evidence required when GTK overlay changes; user previously reported Ubuntu command execution path works",
            ),
        },
    ]
}

fn run_check(
    workspace: &Path,
    run_dir: &Path,
    spec: &CheckSpec,
    mode: ProofMode,
    selection: &ProofSelection,
) -> Result<CheckResult, String> {
    if let Some(result) = run_manual_evidence_check(run_dir, spec, mode)? {
        return Ok(result);
    }

    if let Some(reason) = skip_reason_for_spec(spec, mode, selection) {
        println!("skip {}: {}", spec.id, reason);
        return Ok(CheckResult {
            id: spec.id.into(),
            title: spec.title.into(),
            status: CheckStatus::Skipped,
            command: spec.command.iter().map(|part| (*part).into()).collect(),
            duration_ms: None,
            exit_code: None,
            log_path: None,
            skip_reason: Some(reason),
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

fn run_manual_evidence_check(
    run_dir: &Path,
    spec: &CheckSpec,
    mode: ProofMode,
) -> Result<Option<CheckResult>, String> {
    let Some(env_name) = manual_evidence_env(spec.id) else {
        return Ok(None);
    };
    let Some(raw_path) = env::var_os(env_name).filter(|value| !value.is_empty()) else {
        if mode == ProofMode::Release {
            return Ok(Some(CheckResult {
                id: spec.id.into(),
                title: spec.title.into(),
                status: CheckStatus::Failed,
                command: Vec::new(),
                duration_ms: Some(0),
                exit_code: Some(1),
                log_path: None,
                skip_reason: Some(format!(
                    "required desktop evidence not provided; set {env_name}"
                )),
            }));
        }
        return Ok(None);
    };

    let evidence_path = PathBuf::from(raw_path);
    let exists = evidence_path.exists();
    let status = if exists {
        CheckStatus::Passed
    } else {
        CheckStatus::Failed
    };
    let log_path = run_dir.join("logs").join(format!("{}.log", spec.id));
    write_manual_evidence_log(&log_path, spec, env_name, &evidence_path, exists)?;
    let relative_log_path =
        path_slash(log_path.strip_prefix(run_dir).unwrap_or(log_path.as_path()));

    Ok(Some(CheckResult {
        id: spec.id.into(),
        title: spec.title.into(),
        status,
        command: Vec::new(),
        duration_ms: Some(0),
        exit_code: if exists { Some(0) } else { Some(1) },
        log_path: Some(relative_log_path),
        skip_reason: None,
    }))
}

fn manual_evidence_env(spec_id: &str) -> Option<&'static str> {
    match spec_id {
        "windows-webview2-visual" => Some("ZHONGSHU_WEBVIEW2_EVIDENCE"),
        "ubuntu-gtk-visual" => Some("ZHONGSHU_GTK_EVIDENCE"),
        _ => None,
    }
}

fn write_manual_evidence_log(
    path: &Path,
    spec: &CheckSpec,
    env_name: &str,
    evidence_path: &Path,
    exists: bool,
) -> Result<(), String> {
    let mut text = String::new();
    writeln!(&mut text, "manual evidence check: {}", spec.id).unwrap();
    writeln!(&mut text, "title: {}", spec.title).unwrap();
    writeln!(&mut text, "env: {env_name}").unwrap();
    writeln!(&mut text, "evidence_path: {}", evidence_path.display()).unwrap();
    writeln!(&mut text, "exists: {exists}").unwrap();
    if exists {
        let kind = if evidence_path.is_dir() {
            "directory"
        } else {
            "file"
        };
        writeln!(&mut text, "kind: {kind}").unwrap();
    } else {
        writeln!(&mut text, "error: configured evidence path does not exist").unwrap();
    }
    fs::write(path, text).map_err(|err| format!("cannot write {}: {err}", path.display()))
}

fn skip_reason_for_spec(
    spec: &CheckSpec,
    mode: ProofMode,
    selection: &ProofSelection,
) -> Option<String> {
    if let Some(reason) = spec.skip_reason {
        return Some(reason.to_string());
    }
    if !spec.modes.contains(&mode) {
        return Some(format!("not selected for proof mode '{}'", mode.as_str()));
    }
    if spec.requires_loopback_bind && !selection.loopback_bind_available {
        return Some("loopback TCP bind is unavailable in this environment".into());
    }
    if mode == ProofMode::Local
        && !selection.changed_files.is_empty()
        && !spec.areas.is_empty()
        && !spec
            .areas
            .iter()
            .any(|area| selection.changed_areas.contains(area))
    {
        return Some(format!(
            "not selected for changed areas: {}",
            area_list(&selection.changed_areas)
        ));
    }
    None
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
        Err(err) if program == "corepack" && err.kind() == io::ErrorKind::NotFound => {
            let Some((fallback_program, fallback_args)) = command_args.split_first() else {
                return Err(format!("cannot run '{}': {err}", display_command.join(" ")));
            };
            Command::new(fallback_program)
                .args(fallback_args.iter().map(OsStr::new))
                .current_dir(workspace)
                .output()
                .map_err(|fallback_err| {
                    format!(
                        "cannot run '{}' through corepack ({err}) or directly ({fallback_err})",
                        display_command.join(" ")
                    )
                })
        }
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
    selection: &ProofSelection,
    results: &[CheckResult],
) -> Result<(), String> {
    let summary = summarize(results);
    let mut json = String::new();
    writeln!(&mut json, "{{").unwrap();
    writeln!(&mut json, "  \"schema_version\": 1,").unwrap();
    writeln!(&mut json, "  \"mode\": \"{}\",", mode.as_str()).unwrap();
    writeln!(
        &mut json,
        "  \"mode_policy\": \"{}\",",
        json_escape(mode.policy())
    )
    .unwrap();
    writeln!(&mut json, "  \"generated_at_unix_secs\": {generated_at},").unwrap();
    writeln!(
        &mut json,
        "  \"changed_files\": {},",
        json_string_array(&selection.changed_files)
    )
    .unwrap();
    writeln!(
        &mut json,
        "  \"changed_areas\": {},",
        json_string_array(
            &selection
                .changed_areas
                .iter()
                .map(|area| area.as_str().to_string())
                .collect::<Vec<_>>()
        )
    )
    .unwrap();
    writeln!(
        &mut json,
        "  \"loopback_bind_available\": {},",
        selection.loopback_bind_available
    )
    .unwrap();
    writeln!(
        &mut json,
        "  \"summary\": {{ \"passed\": {}, \"failed\": {}, \"skipped\": {} }},",
        summary.passed, summary.failed, summary.skipped
    )
    .unwrap();
    writeln!(&mut json, "  \"mode_matrix\": [").unwrap();
    for (index, proof_mode) in ProofMode::all().iter().enumerate() {
        let comma = if index + 1 == ProofMode::all().len() {
            ""
        } else {
            ","
        };
        writeln!(
            &mut json,
            "    {{ \"mode\": \"{}\", \"policy\": \"{}\" }}{comma}",
            proof_mode.as_str(),
            json_escape(proof_mode.policy())
        )
        .unwrap();
    }
    writeln!(&mut json, "  ],").unwrap();
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
    selection: &ProofSelection,
    results: &[CheckResult],
) -> Result<(), String> {
    let summary = summarize(results);
    let mut markdown = String::new();
    writeln!(&mut markdown, "# Zhongshu Proof Report").unwrap();
    writeln!(&mut markdown).unwrap();
    writeln!(&mut markdown, "- Mode: `{}`", mode.as_str()).unwrap();
    writeln!(&mut markdown, "- Mode policy: {}", mode.policy()).unwrap();
    writeln!(&mut markdown, "- Generated: `{generated_at}`").unwrap();
    writeln!(
        &mut markdown,
        "- Changed areas: `{}`",
        area_list(&selection.changed_areas)
    )
    .unwrap();
    writeln!(
        &mut markdown,
        "- Changed files: `{}`",
        selection.changed_files.len()
    )
    .unwrap();
    writeln!(
        &mut markdown,
        "- Loopback bind available: `{}`",
        selection.loopback_bind_available
    )
    .unwrap();
    writeln!(&mut markdown, "- Passed: `{}`", summary.passed).unwrap();
    writeln!(&mut markdown, "- Failed: `{}`", summary.failed).unwrap();
    writeln!(&mut markdown, "- Skipped: `{}`", summary.skipped).unwrap();
    writeln!(&mut markdown).unwrap();
    writeln!(&mut markdown, "## Mode Matrix").unwrap();
    writeln!(&mut markdown).unwrap();
    writeln!(&mut markdown, "| Mode | Policy |").unwrap();
    writeln!(&mut markdown, "| --- | --- |").unwrap();
    for proof_mode in ProofMode::all() {
        writeln!(
            &mut markdown,
            "| `{}` | {} |",
            proof_mode.as_str(),
            proof_mode.policy()
        )
        .unwrap();
    }
    writeln!(&mut markdown).unwrap();
    writeln!(&mut markdown, "## Checks").unwrap();
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

fn write_junit_xml(path: &Path, results: &[CheckResult]) -> Result<(), String> {
    let summary = summarize(results);
    let mut xml = String::new();
    writeln!(&mut xml, r#"<?xml version="1.0" encoding="UTF-8"?>"#).unwrap();
    writeln!(
        &mut xml,
        r#"<testsuite name="zhongshu-proof" tests="{}" failures="{}" skipped="{}">"#,
        results.len(),
        summary.failed,
        summary.skipped
    )
    .unwrap();
    for result in results {
        writeln!(
            &mut xml,
            r#"  <testcase classname="proof" name="{}" time="{}">"#,
            xml_escape(&result.id),
            result
                .duration_ms
                .map(|duration| format!("{:.3}", duration as f64 / 1000.0))
                .unwrap_or_else(|| "0".to_string())
        )
        .unwrap();
        match result.status {
            CheckStatus::Failed => {
                writeln!(
                    &mut xml,
                    r#"    <failure message="exit code {:?}">See {}</failure>"#,
                    result.exit_code,
                    xml_escape(result.log_path.as_deref().unwrap_or(""))
                )
                .unwrap();
            }
            CheckStatus::Skipped => {
                writeln!(
                    &mut xml,
                    r#"    <skipped message="{}" />"#,
                    xml_escape(result.skip_reason.as_deref().unwrap_or(""))
                )
                .unwrap();
            }
            CheckStatus::Passed => {}
        }
        writeln!(&mut xml, "  </testcase>").unwrap();
    }
    writeln!(&mut xml, "</testsuite>").unwrap();
    fs::write(path, xml).map_err(|err| format!("cannot write {}: {err}", path.display()))
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

fn area_list(areas: &BTreeSet<ProofArea>) -> String {
    if areas.is_empty() {
        return "none".into();
    }
    areas
        .iter()
        .map(|area| area.as_str())
        .collect::<Vec<_>>()
        .join(", ")
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

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
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
    fn attack_matrix_specs_cover_split_attack_files_in_pr_modes() {
        let specs = proof_check_specs();
        for id in [
            "harness-attack",
            "harness-attack-shell",
            "harness-attack-filesystem",
            "harness-attack-edit",
            "harness-attack-verify",
            "harness-attack-browser",
            "harness-attack-equipment",
        ] {
            let spec = specs.iter().find(|spec| spec.id == id).unwrap_or_else(|| {
                panic!("missing attack proof spec {id}");
            });
            assert!(
                spec.modes.contains(&ProofMode::Pr),
                "{id} should run in PR mode"
            );
            assert!(
                spec.areas.contains(&ProofArea::Harness),
                "{id} should be routed by harness changes"
            );
        }
    }

    #[test]
    fn harness_attack_test_files_classify_as_harness_area() {
        let areas = classify_changed_areas(&[
            "zhongshu-core/tests/harness_attack_shell.rs".into(),
            "zhongshu-core/tests/harness_attack_edit.rs".into(),
            "zhongshu-core/tests/harness_attack_equipment.rs".into(),
        ]);

        assert!(areas.contains(&ProofArea::Harness));
        assert!(areas.contains(&ProofArea::EquipmentMcp));
    }

    #[test]
    fn markdown_report_includes_mode_matrix() {
        let temp_dir = env::temp_dir().join(format!("zhongshu-proof-test-{}", unix_secs()));
        fs::create_dir_all(&temp_dir).unwrap();
        let report_path = temp_dir.join("report.md");
        let mut changed_areas = BTreeSet::new();
        changed_areas.insert(ProofArea::ProofRunner);
        let selection = ProofSelection {
            changed_files: vec!["xtask/src/main.rs".into()],
            changed_areas,
            loopback_bind_available: true,
        };
        let results = vec![check_result("fmt", CheckStatus::Passed)];

        write_report_markdown(&report_path, ProofMode::Pr, 1, &selection, &results).unwrap();

        let report = fs::read_to_string(report_path).unwrap();
        assert!(report.contains("## Mode Matrix"));
        assert!(report.contains("`pr` | routed checks plus full attack matrix"));
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn manual_webview2_evidence_path_marks_visual_check_passed() {
        let temp_dir =
            env::temp_dir().join(format!("zhongshu-proof-evidence-test-{}", unix_secs()));
        let logs_dir = temp_dir.join("logs");
        fs::create_dir_all(&logs_dir).unwrap();
        let evidence_path = temp_dir.join("webview2-smoke.txt");
        fs::write(&evidence_path, "visible window, close hides").unwrap();
        env::set_var("ZHONGSHU_WEBVIEW2_EVIDENCE", &evidence_path);
        let spec = proof_check_specs()
            .into_iter()
            .find(|spec| spec.id == "windows-webview2-visual")
            .unwrap();

        let result = run_manual_evidence_check(&temp_dir, &spec, ProofMode::Local)
            .unwrap()
            .expect("manual evidence result");

        assert_eq!(result.status, CheckStatus::Passed);
        let log_path = temp_dir.join(result.log_path.unwrap());
        let log = fs::read_to_string(log_path).unwrap();
        assert!(log.contains("ZHONGSHU_WEBVIEW2_EVIDENCE"));
        assert!(log.contains("exists: true"));
        env::remove_var("ZHONGSHU_WEBVIEW2_EVIDENCE");
        let _ = fs::remove_dir_all(temp_dir);
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
