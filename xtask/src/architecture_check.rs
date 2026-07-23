use std::fs;
use std::path::Path;

const WORKSPACE_ROOT: &str = "/home/gordon/code/zhongshu/";

struct Rule {
    pattern: &'static str,
    allowed: &'static [&'static str],
    description: &'static str,
}

const RULES: &[Rule] = &[
    Rule {
        pattern: "run_agent_with_context(",
        allowed: &["/loop_.rs", "/entry.rs"],
        description: "run_agent_with_context() must not be called directly; use execute_agent_loop()",
    },
    Rule {
        pattern: "run_agent_with_verification_policy(",
        allowed: &["/loop_.rs", "/worker.rs"],
        description: "run_agent_with_verification_policy() must not be called directly; use execute_agent_loop()",
    },
    Rule {
        pattern: "run_agent(",
        allowed: &["/loop_.rs", "/entry.rs"],
        description: "run_agent() must not be called directly; use execute_agent_loop()",
    },
    Rule {
        pattern: "AgentCallbacks {",
        allowed: &["/app.rs", "/main.rs", "/loop_.rs"],
        description: "AgentCallbacks must only be constructed in app.rs",
    },
];

pub fn run() -> Result<(), String> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let dirs = &[
        workspace_root.join("zhongshu-core/src"),
        workspace_root.join("zhongshu-orb/src"),
        workspace_root.join("zhongshu-cli/src"),
    ];

    let mut errors: Vec<String> = Vec::new();
    for src_dir in dirs {
        scan_dir(src_dir, &mut errors)?;
    }

    if errors.is_empty() {
        println!("architecture-check: OK — no illegal call sites found");
        Ok(())
    } else {
        for e in &errors {
            eprintln!("[FAIL] {e}");
        }
        eprintln!(
            "\narchitecture-check: FAILED — {} violation(s)",
            errors.len()
        );
        Err(format!("{} architecture violation(s)", errors.len()))
    }
}

fn scan_dir(dir: &Path, errors: &mut Vec<String>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("read_dir {dir:?}: {e}"))? {
        let entry = entry.map_err(|e| format!("entry: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            scan_dir(&path, errors)?;
            continue;
        }
        if path.extension().map_or(false, |ext| ext == "rs") {
            check_file(&path, errors)?;
        }
    }
    Ok(())
}

fn check_file(path: &Path, errors: &mut Vec<String>) -> Result<(), String> {
    let full = path.to_string_lossy().replace('\\', "/");
    let relative = full
        .strip_prefix(WORKSPACE_ROOT)
        .unwrap_or(&full)
        .to_string();

    // Skip test helper files
    if relative.contains("/tests/") || relative.contains("test_") {
        return Ok(());
    }

    let content = fs::read_to_string(path).map_err(|e| format!("read {path:?}: {e}"))?;

    for rule in RULES {
        let is_allowed = rule.allowed.iter().any(|a| relative.contains(a));
        if is_allowed {
            continue;
        }
        for (lineno, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with('#') {
                continue;
            }
            if trimmed.contains(rule.pattern) {
                errors.push(format!(
                    "{}:{}: {} (found `{}`)",
                    relative,
                    lineno + 1,
                    rule.description,
                    rule.pattern
                ));
            }
        }
    }
    Ok(())
}
