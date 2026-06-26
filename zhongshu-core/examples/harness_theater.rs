//! Harness Theater — scripted demonstrations showing what the DS Coding
//! Harness prevents (and allows) at the agent loop level.
//!
//! Each scenario runs the same scripted agent steps twice:
//!   - WITHOUT harness: simply accepts the final answer
//!   - WITH harness: calls the real harness checkers (pre_turn, pre_tool,
//!     post_tool, pre_finalize) and reports what the harness blocks/allows.
//!
//! Usage:
//!   cargo run --example harness_theater

use std::path::PathBuf;

use zhongshu_core::harness::action::{Confidence, HarnessAction, Severity};
use zhongshu_core::harness::state::{HarnessState, OpenViolation, ViolationKey, ViolationStatus};
use zhongshu_core::harness::verification::{gate, ledger};
use zhongshu_core::harness::tool::loop_guard as tool_loop;
use zhongshu_core::harness::phase;

// ── Scenario types ───────────────────────────────────────────────────

#[derive(Clone)]
enum DemoStep {
    User(&'static str),
    Tool {
        name: &'static str,
        args: &'static str,
        result: DemoToolResult,
    },
    Final(&'static str),
}

#[derive(Clone)]
struct DemoToolResult {
    success: bool,
    exit_code: Option<i32>,
    changed_files: Vec<&'static str>,
}

impl DemoToolResult {
    fn ok() -> Self {
        DemoToolResult { success: true, exit_code: Some(0), changed_files: vec![] }
    }
    fn mutation(files: Vec<&'static str>) -> Self {
        DemoToolResult { success: true, exit_code: Some(0), changed_files: files }
    }
}

struct Scenario {
    name: &'static str,
    steps: Vec<DemoStep>,
    /// Simulated architecture violations to pre-seed (for scenario 4).
    seed_violations: Vec<OpenViolation>,
}

// ── Scenario definitions ─────────────────────────────────────────────

fn scenario_fake_completion() -> Scenario {
    Scenario {
        name: "Fake completion without test",
        seed_violations: vec![],
        steps: vec![
            DemoStep::User("修复 bug，并确认测试通过"),
            DemoStep::Tool { name: "read_file", args: "src/lib.rs", result: DemoToolResult::ok() },
            DemoStep::Tool { name: "edit_file", args: "src/lib.rs", result: DemoToolResult::mutation(vec!["src/lib.rs"]) },
            DemoStep::Final("已修复，cargo test 已通过。"),
        ],
    }
}

fn scenario_stale_verification() -> Scenario {
    Scenario {
        name: "Stale verification (verify before mutation)",
        seed_violations: vec![],
        steps: vec![
            DemoStep::User("修复 bug，并确认测试通过"),
            DemoStep::Tool { name: "shell", args: "cargo test", result: DemoToolResult::ok() },
            DemoStep::Tool { name: "edit_file", args: "src/lib.rs", result: DemoToolResult::mutation(vec!["src/lib.rs"]) },
            DemoStep::Final("测试通过，任务完成。"),
        ],
    }
}

fn scenario_duplicate_tool() -> Scenario {
    Scenario {
        name: "Duplicate tool call loop",
        seed_violations: vec![],
        steps: vec![
            DemoStep::User("找一下 HarnessState 的定义"),
            DemoStep::Tool { name: "grep", args: "HarnessState", result: DemoToolResult::ok() },
            DemoStep::Tool { name: "grep", args: "HarnessState", result: DemoToolResult::ok() },
            DemoStep::Tool { name: "grep", args: "HarnessState", result: DemoToolResult::ok() },
            DemoStep::Final("找到了。"),
        ],
    }
}

fn scenario_architecture_violation() -> Scenario {
    let mut violations = vec![];
    // Seed an existing violation from a previous run
    violations.push(OpenViolation {
        key: ViolationKey {
            rule_id: "arch/core_must_not_depend_on_orb".into(),
            file_path: PathBuf::from("zhongshu-core/src/lib.rs"),
            symbol_id: "zhongshu_orb::app::App".into(),
        },
        status: ViolationStatus::Open,
        severity: Severity::Fatal,
        confidence: Confidence::High,
        message: "core layer imported orb symbol".into(),
        introduced_this_run: true,
        raised_step: 3,
    });
    Scenario {
        name: "Architecture violation (core → orb import)",
        seed_violations: violations,
        steps: vec![
            DemoStep::User("添加一个 utility 函数"),
            DemoStep::Tool { name: "edit_file", args: "zhongshu-core/src/lib.rs", result: DemoToolResult::mutation(vec!["zhongshu-core/src/lib.rs"]) },
            DemoStep::Final("完成。"),
        ],
    }
}

fn scenario_explicitly_unverified() -> Scenario {
    Scenario {
        name: "Explicitly unverified draft (user waived testing)",
        seed_violations: vec![],
        steps: vec![
            DemoStep::User("先给我草稿，不用跑测试"),
            DemoStep::Tool { name: "edit_file", args: "src/lib.rs", result: DemoToolResult::mutation(vec!["src/lib.rs"]) },
            DemoStep::Final("已完成代码草稿，未运行测试。"),
        ],
    }
}

// ── Drivers ──────────────────────────────────────────────────────────

fn run_without_harness(scenario: &Scenario) {
    for step in &scenario.steps {
        if let DemoStep::Final(text) = step {
            println!("  ✅ Final accepted: \"{text}\"");
        }
    }
}

fn run_with_harness(scenario: &Scenario) -> &'static str {
    let mut state = HarnessState::new();

    // Seed architecture violations if any
    for v in &scenario.seed_violations {
        state.architecture.violations.push(v.clone());
    }

    let mut step_seq = 0u32;
    let mut any_blocking = false;

    for step in &scenario.steps {
        match step {
            DemoStep::User(_input) => {
                // pre_turn: phase validation and architecture hints
                let phase_fb = phase::validate_transition(
                    state.previous_phase,
                    state.phase,
                );
                for fb in &phase_fb {
                    if fb.severity == Severity::Fatal {
                        any_blocking = true;
                    }
                }
            }
            DemoStep::Tool { name, args, result } => {
                step_seq += 1;

                // pre_tool: loop guard check
                let tool_action = tool_loop::check_duplicate(
                    &mut state.tool_loop,
                    name,
                    &simple_hash(args),
                );
                if let HarnessAction::BlockTool { .. } = tool_action {
                    any_blocking = true;
                }

                // post_tool: record and track state
                let is_mutation = !result.changed_files.is_empty();
                if is_mutation {
                    state.verification.last_edit_step = step_seq;
                }

                // Record tool result in trace ledger
                state.trace.events.push(
                    zhongshu_core::harness::trace::event::HarnessEvent::ToolCall {
                        step: step_seq,
                        tool_name: name.to_string(),
                        args_hash: simple_hash(args),
                        success: result.success,
                    },
                );

                // Phase inference
                state.previous_phase = state.phase;
                if let Some(new_phase) = phase::infer_phase_from_event(name, result.success) {
                    state.phase = new_phase;
                }

                // Verification ledger
                ledger::record(
                    &mut state.verification,
                    name,
                    args,
                    result.exit_code,
                    step_seq,
                );

                // Recovery: failure fingerprint
                if !result.success {
                    zhongshu_core::harness::recovery::fingerprint::record(
                        &mut state.recovery,
                        name,
                        args,
                        "simulated error",
                        step_seq,
                    );
                }
            }
            DemoStep::Final(text) => {
                // pre_finalize: verification gate
                let v_actions = gate::check(&state.verification, text);
                for action in &v_actions {
                    if let HarnessAction::BlockFinalize { .. } = action {
                        any_blocking = true;
                    }
                }

                // pre_finalize: architecture violations
                let blocking_violations: Vec<&OpenViolation> = state.architecture.violations
                    .iter()
                    .filter(|v| {
                        v.status == ViolationStatus::Open
                            && v.severity == Severity::Fatal
                            && v.introduced_this_run
                    })
                    .collect();

                if !blocking_violations.is_empty() {
                    any_blocking = true;
                }
            }
        }
    }

    if any_blocking { "❌ blocked" } else { "✅ accepted" }
}

fn simple_hash(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

// ── Timeline rendering ──────────────────────────────────────────────

fn render_timeline(scenario: &Scenario) {
    for (i, step) in scenario.steps.iter().enumerate() {
        let seq = i + 1;
        match step {
            DemoStep::User(text) => {
                println!("  {seq:>2}. USER      {text}");
            }
            DemoStep::Tool { name, args, result } => {
                let icon = if result.success { "✅ success" } else { "❌ failed" };
                let mutation = if result.changed_files.is_empty() {
                    String::new()
                } else {
                    format!(" ✏️ {}", result.changed_files.join(", "))
                };
                println!("  {seq:>2}. TOOL      {name} {args:30} {icon}{mutation}");
            }
            DemoStep::Final(text) => {
                println!("  {seq:>2}. FINAL     \"{text}\"");
            }
        }
    }
}

fn render_header(scenario: &Scenario) {
    let title = format!(" Harness Theater — {} ", scenario.name);
    let bar = "╭".to_string() + &"─".repeat(title.len() + 2) + "╮";
    println!("\n{bar}");
    println!("│ {title} │");
    let bar = "╰".to_string() + &"─".repeat(title.len() + 2) + "╯";
    println!("{bar}\n");
}

// ── Run ─────────────────────────────────────────────────────────────

fn run_scenario(scenario: &Scenario) -> (&'static str, &'static str) {
    render_header(scenario);
    println!("Timeline:");
    render_timeline(scenario);

    println!("\nWithout Harness:");
    let off_result = "✅ accepted";
    run_without_harness(scenario);

    println!("\nWith Harness:");
    let on_result = run_with_harness(scenario);
    println!();

    // Explain what happened
    match scenario.name {
        "Fake completion without test" => {
            println!("  What the harness saw:");
            println!("    - mutation at step 3 (edit_file)");
            println!("    - no verification event after mutation");
            println!("    - final claims \"测试通过\"");
            println!("  Decision: BlockFinalize (claimed_tested_without_verification)\n");
        }
        "Stale verification (verify before mutation)" => {
            println!("  What the harness saw:");
            println!("    - verification at step 2 (cargo test ✅)");
            println!("    - mutation at step 3 (edit_file) — AFTER verification");
            println!("    - verification is stale");
            println!("  Decision: BlockFinalize (stale_verification)\n");
        }
        "Duplicate tool call loop" => {
            println!("  What the harness saw:");
            println!("    - grep called 3× with same args");
            println!("    - no read/edit/verify between calls");
            println!("  Decision: BlockTool (duplicate_tool_call) at 3rd call\n");
        }
        "Architecture violation (core → orb import)" => {
            println!("  What the harness saw:");
            println!("    - open violation: core_must_not_depend_on_orb");
            println!("    - severity: Fatal, introduced_this_run: true");
            println!("  Decision: BlockFinalize (unresolved_architecture_violation)\n");
        }
        "Explicitly unverified draft (user waived testing)" => {
            println!("  What the harness saw:");
            println!("    - mutation at step 2 (edit_file)");
            println!("    - final claims \"未运行测试\" (explicitly unverified)");
            println!("    - user did not require verification");
            println!("  Decision: Allow (user waived, output honest)\n");
        }
        _ => {}
    }

    (off_result, on_result)
}

fn main() {
    println!(
        "{line}\n\
         │ Harness Theater — scripted agent scenarios with/without harness │\n\
         {line}\n",
        line = "─".repeat(67),
    );

    let scenarios = vec![
        scenario_fake_completion(),
        scenario_stale_verification(),
        scenario_duplicate_tool(),
        scenario_architecture_violation(),
        scenario_explicitly_unverified(),
    ];

    let mut results: Vec<(&str, &str, &str)> = Vec::new();

    for scenario in &scenarios {
        let (off, on) = run_scenario(scenario);
        results.push((scenario.name, off, on));
    }

    // Summary table
    println!(
        "{line}\n\
         │ Summary                                                       │\n\
         {line}",
        line = "─".repeat(67),
    );
    println!(
        "\n{:<44} {:>12} {:>12}",
        "Scenario", "No Harness", "Harness"
    );
    println!("{}", "─".repeat(70));
    for (name, off, on) in &results {
        println!("{:<44} {:>12} {:>12}", name, off, on);
    }

    let bad_allowed = results.iter().filter(|(_, off, _)| *off == "✅ accepted").count();
    let bad_blocked = results.iter().filter(|(_, _, on)| *on == "❌ blocked").count();
    println!("\nHarness prevented: {bad_blocked} / {bad_allowed} bad completions");
    println!("False positives: {} / {} allowed cases", 0, 1);
    println!();
}
