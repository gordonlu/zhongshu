use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use zhongshu_core::agent::llm::{
    ChatCompletionRequest, ChatCompletionResponse, LlmProvider, OpenAiProvider, StreamEvent,
};
use zhongshu_core::agent::llm_registry::{LlmClient, LlmRegistry};
use zhongshu_core::agent::{
    AgentBudget, AgentProfile, AgentRuntime, Orchestrator, Worker, WorkerExecutionStatus,
};
use zhongshu_core::harness::trace::event::HarnessEvent;
use zhongshu_core::integration::{
    DeeplosslessBenchmarkSnapshot, DeeplosslessConfig, DeeplosslessProxy,
};
use zhongshu_core::task::Task;
use zhongshu_core::tool::{default_registry, Tool, ToolOutput, ToolStatus};

#[derive(Debug, Clone, Deserialize)]
struct Suite {
    id: String,
    cases: Vec<Case>,
}

#[derive(Debug, Clone, Deserialize)]
struct Case {
    id: String,
    title: String,
    fixture: String,
    prompt: String,
    expected_keywords: Vec<String>,
    #[serde(default)]
    forbidden_keywords: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Variant {
    SinglePro,
    LeadTwoWorkers,
    SingleFlash,
}

impl Variant {
    const ALL: [Self; 3] = [Self::SinglePro, Self::LeadTwoWorkers, Self::SingleFlash];

    fn as_str(self) -> &'static str {
        match self {
            Self::SinglePro => "single_pro",
            Self::LeadTwoWorkers => "lead_two_workers",
            Self::SingleFlash => "single_flash",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "single_pro" => Ok(Self::SinglePro),
            "lead_two_workers" => Ok(Self::LeadTwoWorkers),
            "single_flash" => Ok(Self::SingleFlash),
            _ => Err(format!(
                "unknown benchmark variant '{value}'; expected single_pro, lead_two_workers, or single_flash"
            )),
        }
    }
}

#[derive(Debug)]
struct Args {
    suite: PathBuf,
    output: PathBuf,
    repeats: usize,
    live: bool,
    api_key_env: String,
    upstream: String,
    upstream_path: Option<String>,
    flash_api_key_env: Option<String>,
    pro_api_key_env: Option<String>,
    flash_upstream: Option<String>,
    pro_upstream: Option<String>,
    flash_upstream_path: Option<String>,
    pro_upstream_path: Option<String>,
    flash_model: Option<String>,
    pro_model: Option<String>,
    case_filter: Option<String>,
    variant_filter: Option<Variant>,
    max_requests: u64,
    max_tokens: u64,
    max_elapsed_secs: u64,
}

#[derive(Debug, Serialize)]
struct DryRunReport {
    schema_version: u32,
    suite_id: String,
    mode: &'static str,
    repeats: usize,
    planned_trials: usize,
    live_limits: LiveLimits,
    cases: Vec<DryRunCase>,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct LiveLimits {
    max_requests: u64,
    max_tokens: u64,
    max_elapsed_secs: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct LiveBudgetSnapshot {
    limits: LiveLimits,
    admitted_requests: u64,
    provider_reported_tokens: u64,
    elapsed_ms: u128,
}

#[derive(Debug, Serialize)]
struct AbortReport {
    schema_version: u32,
    suite_id: String,
    case_id: String,
    variant: Variant,
    trial: usize,
    error: String,
    live_budget: LiveBudgetSnapshot,
    completed_trials: usize,
    evidence_note: &'static str,
}

impl Default for LiveLimits {
    fn default() -> Self {
        Self {
            max_requests: 12,
            max_tokens: 40_000,
            max_elapsed_secs: 180,
        }
    }
}

#[derive(Debug, Serialize)]
struct DryRunCase {
    id: String,
    title: String,
    fixture: String,
}

#[derive(Debug, Serialize)]
struct TrialResult {
    schema_version: u32,
    suite_id: String,
    case_id: String,
    variant: Variant,
    trial: usize,
    flash_model: String,
    pro_model: String,
    elapsed_ms: u128,
    live_limits: LiveLimits,
    status: String,
    outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    collaboration: Option<CollaborationFacts>,
    score: TrialScore,
    provider_facts: Vec<ProviderFacts>,
    report: String,
}

#[derive(Debug, Serialize)]
struct CollaborationFacts {
    analyst_outcome: String,
    verifier_outcome: String,
    lead_summary_succeeded: bool,
    acceptance_reasons: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ProviderFacts {
    role_group: String,
    upstream: String,
    upstream_path: String,
    model: String,
    deeplossless_db: String,
    deeplossless: DeeplosslessBenchmarkSnapshot,
    provider_usage: ProviderUsageSnapshot,
}

#[derive(Debug, Clone, Copy, Serialize, Default)]
struct ProviderUsageSnapshot {
    requests: u64,
    successful_responses: u64,
    failed_requests: u64,
    responses_missing_usage: u64,
    prompt_tokens: u64,
    completion_tokens: u64,
}

impl ProviderUsageSnapshot {
    fn total_tokens(self) -> u64 {
        self.prompt_tokens.saturating_add(self.completion_tokens)
    }
}

#[derive(Default)]
struct ProviderUsageMeter {
    requests: AtomicU64,
    successful_responses: AtomicU64,
    failed_requests: AtomicU64,
    responses_missing_usage: AtomicU64,
    prompt_tokens: AtomicU64,
    completion_tokens: AtomicU64,
}

struct LiveBudgetGuard {
    limits: LiveLimits,
    started: Instant,
    admitted_requests: AtomicU64,
    provider_tokens: AtomicU64,
}

impl LiveBudgetGuard {
    fn new(limits: LiveLimits) -> Self {
        Self {
            limits,
            started: Instant::now(),
            admitted_requests: AtomicU64::new(0),
            provider_tokens: AtomicU64::new(0),
        }
    }

    fn admit_request(&self) -> anyhow::Result<Duration> {
        let reported_tokens = self.provider_tokens.load(Ordering::Relaxed);
        if reported_tokens >= self.limits.max_tokens {
            anyhow::bail!(
                "live benchmark token limit {} reached ({} provider-reported tokens)",
                self.limits.max_tokens,
                reported_tokens
            );
        }
        let elapsed = self.started.elapsed();
        let max_elapsed = Duration::from_secs(self.limits.max_elapsed_secs);
        let remaining = max_elapsed.checked_sub(elapsed).ok_or_else(|| {
            anyhow::anyhow!(
                "live benchmark elapsed-time limit {}s reached before provider call",
                self.limits.max_elapsed_secs
            )
        })?;
        self.admitted_requests
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                (current < self.limits.max_requests).then_some(current + 1)
            })
            .map_err(|current| {
                anyhow::anyhow!(
                    "live benchmark request limit {} reached ({} calls already admitted)",
                    self.limits.max_requests,
                    current
                )
            })?;
        Ok(remaining)
    }

    fn record_tokens(&self, tokens: u64) -> anyhow::Result<()> {
        let previous = self.provider_tokens.fetch_add(tokens, Ordering::Relaxed);
        let total = previous.saturating_add(tokens);
        if total > self.limits.max_tokens {
            anyhow::bail!(
                "live benchmark token limit {} exceeded after provider reported {} total tokens; no further provider calls will run",
                self.limits.max_tokens,
                total
            );
        }
        Ok(())
    }

    fn snapshot(&self) -> LiveBudgetSnapshot {
        LiveBudgetSnapshot {
            limits: self.limits,
            admitted_requests: self.admitted_requests.load(Ordering::Relaxed),
            provider_reported_tokens: self.provider_tokens.load(Ordering::Relaxed),
            elapsed_ms: self.started.elapsed().as_millis(),
        }
    }

    fn clamp_request_tokens(&self, request: &mut ChatCompletionRequest) {
        let reported = self.provider_tokens.load(Ordering::Relaxed);
        let remaining = self.limits.max_tokens.saturating_sub(reported);
        let cap = remaining.min(u32::MAX as u64) as u32;
        request.max_tokens = Some(
            request
                .max_tokens
                .map_or(cap, |requested| requested.min(cap)),
        );
    }
}

#[derive(Default)]
struct ToolPolicyMeter {
    test_calls: AtomicU64,
    violations: AtomicU64,
}

struct BenchmarkReadTool {
    policy_meter: Arc<ToolPolicyMeter>,
}

#[async_trait::async_trait]
impl Tool for BenchmarkReadTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read one fixture file. The only accepted paths are exactly `./Cargo.toml` and `./src/lib.rs`; absolute paths and other files are rejected."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "enum": ["./Cargo.toml", "./src/lib.rs"]
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let Some(path) = allowed_benchmark_read_path(arguments) else {
            self.policy_meter.violations.fetch_add(1, Ordering::Relaxed);
            return ToolOutput::error(
                "benchmark policy violation: read_file only accepts `./Cargo.toml` or `./src/lib.rs`",
            );
        };
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) => return ToolOutput::error(format!("cannot read {path}: {error}")),
        };
        let total_lines = content.lines().count();
        ToolOutput::success(serde_json::json!({
            "path": path,
            "content": content,
            "total_lines": total_lines,
        }))
    }
}

fn allowed_benchmark_read_path(arguments: &serde_json::Value) -> Option<&str> {
    let object = arguments.as_object()?;
    if object.len() != 1 {
        return None;
    }
    match object.get("path")?.as_str()? {
        path @ ("./Cargo.toml" | "./src/lib.rs") => Some(path),
        _ => None,
    }
}

struct BenchmarkTestTool {
    policy_meter: Arc<ToolPolicyMeter>,
}

#[async_trait::async_trait]
impl Tool for BenchmarkTestTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Run the fixture's existing test suite. The only accepted command is exactly `cargo test`; arbitrary shell commands, redirection, cwd overrides, and extra arguments are rejected."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "enum": ["cargo test"],
                    "description": "Must be exactly `cargo test`."
                }
            },
            "required": ["command"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        if !is_allowed_benchmark_test_call(arguments) {
            self.policy_meter.violations.fetch_add(1, Ordering::Relaxed);
            return ToolOutput::error(
                "benchmark policy violation: only the exact command `cargo test` is allowed",
            );
        }
        if self.policy_meter.test_calls.fetch_add(1, Ordering::Relaxed) > 0 {
            self.policy_meter.violations.fetch_add(1, Ordering::Relaxed);
            return ToolOutput::error(
                "benchmark policy violation: `cargo test` may be run at most once per trial",
            );
        }

        let output = tokio::task::spawn_blocking(|| {
            std::process::Command::new("cargo").arg("test").output()
        })
        .await;
        let output = match output {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                return ToolOutput::error(format!("cannot execute cargo test: {error}"));
            }
            Err(error) => {
                return ToolOutput::error(format!("cargo test task failed: {error}"));
            }
        };
        let exit_code = output.status.code().unwrap_or(-1);
        let data = serde_json::json!({
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
            "exit_code": exit_code,
        });
        if output.status.success() {
            ToolOutput::success(data)
        } else {
            ToolOutput {
                status: ToolStatus::Error,
                data: Some(data),
                error: Some(format!("cargo test failed with exit code {exit_code}")),
                auth_program: None,
                auth_command: None,
                external_source: false,
                request_id: None,
            }
        }
    }
}

fn is_allowed_benchmark_test_call(arguments: &serde_json::Value) -> bool {
    arguments.as_object().is_some_and(|object| {
        object.len() == 1
            && object.get("command").and_then(serde_json::Value::as_str) == Some("cargo test")
    })
}

impl ProviderUsageMeter {
    fn snapshot(&self) -> ProviderUsageSnapshot {
        ProviderUsageSnapshot {
            requests: self.requests.load(Ordering::Relaxed),
            successful_responses: self.successful_responses.load(Ordering::Relaxed),
            failed_requests: self.failed_requests.load(Ordering::Relaxed),
            responses_missing_usage: self.responses_missing_usage.load(Ordering::Relaxed),
            prompt_tokens: self.prompt_tokens.load(Ordering::Relaxed),
            completion_tokens: self.completion_tokens.load(Ordering::Relaxed),
        }
    }
}

struct MeteredProvider {
    inner: Arc<dyn LlmProvider>,
    meter: Arc<ProviderUsageMeter>,
    live_budget: Arc<LiveBudgetGuard>,
}

#[async_trait::async_trait]
impl LlmProvider for MeteredProvider {
    async fn chat(
        &self,
        mut request: ChatCompletionRequest,
    ) -> anyhow::Result<ChatCompletionResponse> {
        let remaining = self.live_budget.admit_request()?;
        self.live_budget.clamp_request_tokens(&mut request);
        self.meter.requests.fetch_add(1, Ordering::Relaxed);
        let provider_result = tokio::time::timeout(remaining, self.inner.chat(request))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "live benchmark elapsed-time limit {}s reached during provider call",
                    self.live_budget.limits.max_elapsed_secs
                )
            });
        match provider_result {
            Ok(Ok(response)) => {
                self.meter
                    .successful_responses
                    .fetch_add(1, Ordering::Relaxed);
                if let Some(usage) = &response.usage {
                    self.meter
                        .prompt_tokens
                        .fetch_add(usage.prompt_tokens, Ordering::Relaxed);
                    self.meter
                        .completion_tokens
                        .fetch_add(usage.completion_tokens, Ordering::Relaxed);
                    let response_tokens =
                        usage.prompt_tokens.saturating_add(usage.completion_tokens);
                    if response_tokens == 0 {
                        return Err(anyhow::anyhow!(
                            "provider response reported zero token usage; stopping cost-blind benchmark"
                        ));
                    }
                    self.live_budget.record_tokens(response_tokens)?;
                } else {
                    self.meter
                        .responses_missing_usage
                        .fetch_add(1, Ordering::Relaxed);
                    return Err(anyhow::anyhow!(
                        "provider response omitted usage; stopping cost-blind benchmark"
                    ));
                }
                Ok(response)
            }
            Ok(Err(error)) | Err(error) => {
                self.meter.failed_requests.fetch_add(1, Ordering::Relaxed);
                Err(error)
            }
        }
    }

    async fn stream_chat(
        &self,
        _request: ChatCompletionRequest,
        _on_event: Box<dyn FnMut(StreamEvent) + Send>,
    ) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "live benchmark streaming is disabled because response usage cannot be accounted before cost is incurred"
        ))
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }

    fn change_model(&self, model: &str) -> Arc<dyn LlmProvider> {
        Arc::new(Self {
            inner: self.inner.change_model(model),
            meter: self.meter.clone(),
            live_budget: self.live_budget.clone(),
        })
    }

    async fn embed(&self, input: &str) -> anyhow::Result<Vec<f32>> {
        self.inner.embed(input).await
    }
}

#[derive(Debug, Serialize)]
struct TrialScore {
    expected_found: usize,
    expected_total: usize,
    forbidden_found: Vec<String>,
    keyword_recall: f64,
    terminal_completed: bool,
    report_is_final: bool,
    content_rubric_passed: bool,
    recovery_succeeded: bool,
    tool_policy_compliant: bool,
    tool_policy_violations: u64,
    passed: bool,
}

pub fn run(raw: &[String]) -> Result<(), String> {
    let mut args = parse_args(raw)?;
    args.output = absolute(&args.output)?;
    let suite_path = absolute(&args.suite)?;
    let suite_dir = suite_path
        .parent()
        .ok_or_else(|| "suite path has no parent".to_string())?;
    let suite: Suite = serde_json::from_str(
        &fs::read_to_string(&suite_path)
            .map_err(|error| format!("cannot read {}: {error}", suite_path.display()))?,
    )
    .map_err(|error| format!("invalid suite {}: {error}", suite_path.display()))?;
    validate_suite(&suite, suite_dir)?;
    let cases = selected_cases(&suite, args.case_filter.as_deref())?;
    let variants = selected_variants(args.variant_filter);

    fs::create_dir_all(&args.output)
        .map_err(|error| format!("cannot create {}: {error}", args.output.display()))?;
    if !args.live {
        return write_dry_run(&args, &suite, suite_dir, &cases, &variants);
    }
    validate_live_invocation(&args)?;

    let flash_model = args
        .flash_model
        .clone()
        .ok_or_else(|| "--live requires --flash-model".to_string())?;
    let pro_model = args
        .pro_model
        .clone()
        .ok_or_else(|| "--live requires --pro-model".to_string())?;
    let flash_key_env = args
        .flash_api_key_env
        .as_deref()
        .unwrap_or(&args.api_key_env);
    let pro_key_env = args.pro_api_key_env.as_deref().unwrap_or(&args.api_key_env);
    let flash_api_key = required_key(flash_key_env)?;
    let pro_api_key = required_key(pro_key_env)?;

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| format!("cannot create benchmark runtime: {error}"))?;
    let live_budget = Arc::new(LiveBudgetGuard::new(LiveLimits {
        max_requests: args.max_requests,
        max_tokens: args.max_tokens,
        max_elapsed_secs: args.max_elapsed_secs,
    }));
    let mut results = Vec::new();
    let planned_trials = cases.len() * variants.len() * args.repeats;
    for case in cases {
        for &variant in &variants {
            for trial in 1..=args.repeats {
                println!(
                    "[{}/{}] starting case={} variant={} trial={}",
                    results.len() + 1,
                    planned_trials,
                    case.id,
                    variant.as_str(),
                    trial
                );
                let _ = std::io::stdout().flush();
                let result = runtime.block_on(run_trial(
                    &args,
                    &suite.id,
                    suite_dir,
                    case,
                    variant,
                    trial,
                    &flash_api_key,
                    &pro_api_key,
                    &flash_model,
                    &pro_model,
                    &live_budget,
                ));
                let result = match result {
                    Ok(result) => result,
                    Err(error) => {
                        let abort_report = write_abort_report(
                            &args.output,
                            &suite.id,
                            case,
                            variant,
                            trial,
                            &error,
                            live_budget.snapshot(),
                            results.len(),
                        );
                        return match abort_report {
                            Ok(abort_path) => Err(format!(
                                "{error}; benchmark abort report: {}",
                                abort_path.display()
                            )),
                            Err(report_error) => Err(format!(
                                "{error}; additionally failed to write benchmark abort report: {report_error}"
                            )),
                        };
                    }
                };
                println!(
                    "[{}/{}] finished status={} elapsed_ms={} keyword_recall={:.2}",
                    results.len() + 1,
                    planned_trials,
                    result.status,
                    result.elapsed_ms,
                    result.score.keyword_recall
                );
                write_trial(&args.output, &result)?;
                results.push(result);
            }
        }
    }
    write_summary(&args.output, &suite.id, &results)?;
    println!(
        "benchmark report: {} ({} real-provider trials)",
        args.output.display(),
        results.len()
    );
    Ok(())
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut suite = None;
    let mut output = PathBuf::from(".roadmap/project-review-2026-07-17/benchmarks/latest");
    let mut repeats = 3usize;
    let mut live = false;
    let mut api_key_env = "DEEPSEEK_API_KEY".to_string();
    let mut upstream = "https://api.deepseek.com".to_string();
    let mut upstream_path = None;
    let mut flash_api_key_env = None;
    let mut pro_api_key_env = None;
    let mut flash_upstream = None;
    let mut pro_upstream = None;
    let mut flash_upstream_path = None;
    let mut pro_upstream_path = None;
    let mut flash_model = None;
    let mut pro_model = None;
    let mut case_filter = None;
    let mut variant_filter = None;
    let defaults = LiveLimits::default();
    let mut max_requests = defaults.max_requests;
    let mut max_tokens = defaults.max_tokens;
    let mut max_elapsed_secs = defaults.max_elapsed_secs;
    let mut index = 0;
    while index < raw.len() {
        let value = |name: &str, index: usize| {
            raw.get(index + 1)
                .cloned()
                .ok_or_else(|| format!("{name} requires a value"))
        };
        match raw[index].as_str() {
            "--suite" => {
                suite = Some(PathBuf::from(value("--suite", index)?));
                index += 2;
            }
            "--output" => {
                output = PathBuf::from(value("--output", index)?);
                index += 2;
            }
            "--repeats" => {
                repeats = value("--repeats", index)?
                    .parse()
                    .map_err(|_| "--repeats must be a positive integer".to_string())?;
                if repeats == 0 {
                    return Err("--repeats must be positive".into());
                }
                index += 2;
            }
            "--api-key-env" => {
                api_key_env = value("--api-key-env", index)?;
                index += 2;
            }
            "--upstream" => {
                upstream = value("--upstream", index)?;
                index += 2;
            }
            "--upstream-path" => {
                upstream_path = Some(value("--upstream-path", index)?);
                index += 2;
            }
            "--flash-api-key-env" => {
                flash_api_key_env = Some(value("--flash-api-key-env", index)?);
                index += 2;
            }
            "--pro-api-key-env" => {
                pro_api_key_env = Some(value("--pro-api-key-env", index)?);
                index += 2;
            }
            "--flash-upstream" => {
                flash_upstream = Some(value("--flash-upstream", index)?);
                index += 2;
            }
            "--pro-upstream" => {
                pro_upstream = Some(value("--pro-upstream", index)?);
                index += 2;
            }
            "--flash-upstream-path" => {
                flash_upstream_path = Some(value("--flash-upstream-path", index)?);
                index += 2;
            }
            "--pro-upstream-path" => {
                pro_upstream_path = Some(value("--pro-upstream-path", index)?);
                index += 2;
            }
            "--flash-model" => {
                flash_model = Some(value("--flash-model", index)?);
                index += 2;
            }
            "--pro-model" => {
                pro_model = Some(value("--pro-model", index)?);
                index += 2;
            }
            "--case" => {
                case_filter = Some(value("--case", index)?);
                index += 2;
            }
            "--variant" => {
                variant_filter = Some(Variant::parse(&value("--variant", index)?)?);
                index += 2;
            }
            "--max-requests" => {
                max_requests = positive_u64("--max-requests", &value("--max-requests", index)?)?;
                index += 2;
            }
            "--max-tokens" => {
                max_tokens = positive_u64("--max-tokens", &value("--max-tokens", index)?)?;
                index += 2;
            }
            "--max-elapsed-secs" => {
                max_elapsed_secs =
                    positive_u64("--max-elapsed-secs", &value("--max-elapsed-secs", index)?)?;
                index += 2;
            }
            "--live" => {
                live = true;
                index += 1;
            }
            "--dry-run" => {
                live = false;
                index += 1;
            }
            other => return Err(format!("unknown benchmark argument '{other}'")),
        }
    }
    Ok(Args {
        suite: suite.ok_or_else(|| "benchmark requires --suite".to_string())?,
        output,
        repeats,
        live,
        api_key_env,
        upstream,
        upstream_path,
        flash_api_key_env,
        pro_api_key_env,
        flash_upstream,
        pro_upstream,
        flash_upstream_path,
        pro_upstream_path,
        flash_model,
        pro_model,
        case_filter,
        variant_filter,
        max_requests,
        max_tokens,
        max_elapsed_secs,
    })
}

fn positive_u64(name: &str, value: &str) -> Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("{name} must be a positive integer"))?;
    if parsed == 0 {
        return Err(format!("{name} must be positive"));
    }
    Ok(parsed)
}

fn validate_live_invocation(args: &Args) -> Result<(), String> {
    if args.case_filter.is_none() || args.variant_filter.is_none() || args.repeats != 1 {
        return Err(
            "live benchmark safety gate requires --case, --variant, and --repeats 1; matrix runs stay disabled until offline qualification passes"
                .into(),
        );
    }
    Ok(())
}

fn selected_cases<'a>(
    suite: &'a Suite,
    case_filter: Option<&str>,
) -> Result<Vec<&'a Case>, String> {
    match case_filter {
        Some(case_id) => suite
            .cases
            .iter()
            .find(|case| case.id == case_id)
            .map(|case| vec![case])
            .ok_or_else(|| format!("unknown benchmark case '{case_id}'")),
        None => Ok(suite.cases.iter().collect()),
    }
}

fn selected_variants(variant_filter: Option<Variant>) -> Vec<Variant> {
    variant_filter
        .map(|variant| vec![variant])
        .unwrap_or_else(|| Variant::ALL.to_vec())
}

fn required_key(name: &str) -> Result<String, String> {
    let key = env::var(name).map_err(|_| format!("--live requires env {name}"))?;
    if key.trim().is_empty() {
        Err(format!("env {name} is empty"))
    } else {
        Ok(key)
    }
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|error| format!("cannot resolve current directory: {error}"))
    }
}

fn validate_suite(suite: &Suite, suite_dir: &Path) -> Result<(), String> {
    if suite.id.trim().is_empty() || suite.cases.is_empty() {
        return Err("suite requires a non-empty id and at least one case".into());
    }
    for case in &suite.cases {
        if case.id.trim().is_empty()
            || case.prompt.trim().is_empty()
            || case.expected_keywords.is_empty()
        {
            return Err(format!(
                "case '{}' requires id, prompt, and expected_keywords",
                case.id
            ));
        }
        let fixture = suite_dir.join(&case.fixture);
        if !fixture.is_dir() {
            return Err(format!("fixture does not exist: {}", fixture.display()));
        }
    }
    Ok(())
}

fn write_dry_run(
    args: &Args,
    suite: &Suite,
    suite_dir: &Path,
    cases: &[&Case],
    variants: &[Variant],
) -> Result<(), String> {
    let report = DryRunReport {
        schema_version: 1,
        suite_id: suite.id.clone(),
        mode: "dry_run_no_provider_calls",
        repeats: args.repeats,
        planned_trials: cases.len() * variants.len() * args.repeats,
        live_limits: LiveLimits {
            max_requests: args.max_requests,
            max_tokens: args.max_tokens,
            max_elapsed_secs: args.max_elapsed_secs,
        },
        cases: cases
            .iter()
            .map(|case| DryRunCase {
                id: case.id.clone(),
                title: case.title.clone(),
                fixture: suite_dir.join(&case.fixture).display().to_string(),
            })
            .collect(),
    };
    let path = args.output.join("dry-run.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    println!(
        "benchmark dry-run: {} ({} planned trials, no provider calls)",
        path.display(),
        report.planned_trials
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_trial(
    args: &Args,
    suite_id: &str,
    suite_dir: &Path,
    case: &Case,
    variant: Variant,
    trial: usize,
    flash_api_key: &str,
    pro_api_key: &str,
    flash_model: &str,
    pro_model: &str,
    live_budget: &Arc<LiveBudgetGuard>,
) -> Result<TrialResult, String> {
    let trial_dir = TempDir::new().map_err(|error| format!("cannot create trial dir: {error}"))?;
    let workspace = trial_dir.path().join("workspace");
    copy_dir(&suite_dir.join(&case.fixture), &workspace)?;
    let facts_dir = args.output.join("deeplossless").join(format!(
        "{}-{}-{}",
        case.id,
        variant.as_str(),
        trial
    ));
    fs::create_dir_all(&facts_dir)
        .map_err(|error| format!("cannot create {}: {error}", facts_dir.display()))?;
    let flash_upstream = args.flash_upstream.as_deref().unwrap_or(&args.upstream);
    let pro_upstream = args.pro_upstream.as_deref().unwrap_or(&args.upstream);
    let flash_upstream_path = resolved_upstream_path(
        args.flash_upstream_path.as_deref(),
        args.upstream_path.as_deref(),
        flash_upstream,
    );
    let pro_upstream_path = resolved_upstream_path(
        args.pro_upstream_path.as_deref(),
        args.upstream_path.as_deref(),
        pro_upstream,
    );
    let mut flash_proxy = start_trial_proxy(
        &facts_dir,
        "workers",
        flash_upstream,
        flash_upstream_path,
        flash_api_key,
        flash_model,
    )
    .await?;
    let mut pro_proxy = start_trial_proxy(
        &facts_dir,
        "lead",
        pro_upstream,
        pro_upstream_path,
        pro_api_key,
        pro_model,
    )
    .await?;

    let flash_usage = Arc::new(ProviderUsageMeter::default());
    let pro_usage = Arc::new(ProviderUsageMeter::default());
    let flash_inner: Arc<dyn LlmProvider> = Arc::new(
        OpenAiProvider::new(flash_api_key, flash_model).with_base_url(flash_proxy.proxy.base_url()),
    );
    let pro_inner: Arc<dyn LlmProvider> = Arc::new(
        OpenAiProvider::new(pro_api_key, pro_model).with_base_url(pro_proxy.proxy.base_url()),
    );
    let flash_provider: Arc<dyn LlmProvider> = Arc::new(MeteredProvider {
        inner: flash_inner,
        meter: flash_usage.clone(),
        live_budget: live_budget.clone(),
    });
    let pro_provider: Arc<dyn LlmProvider> = Arc::new(MeteredProvider {
        inner: pro_inner,
        meter: pro_usage.clone(),
        live_budget: live_budget.clone(),
    });
    let tool_policy_meter = Arc::new(ToolPolicyMeter::default());
    // Override the production shell for benchmark runs. Prompt instructions are
    // not an enforcement boundary: this tool can only launch the fixture's
    // existing tests and cannot interpret shell syntax or write helper scripts.
    let registry = default_registry()
        .register(BenchmarkReadTool {
            policy_meter: tool_policy_meter.clone(),
        })
        .register(BenchmarkTestTool {
            policy_meter: tool_policy_meter.clone(),
        });
    let (runtime_provider, runtime_model) = if matches!(variant, Variant::SinglePro) {
        (pro_provider.clone(), pro_model)
    } else {
        (flash_provider.clone(), flash_model)
    };
    let runtime = AgentRuntime::with_llm(
        runtime_provider,
        runtime_model.to_string(),
        registry,
        AgentBudget::assistant_default(),
    );
    let _cwd = CurrentDirGuard::enter(&workspace)?;
    let started = Instant::now();
    let (
        report,
        scored_report,
        status,
        outcome,
        collaboration,
        recovery_candidate,
        disallowed_tool_calls,
    ) = match variant {
        Variant::SinglePro | Variant::SingleFlash => {
            let model = match variant {
                Variant::SinglePro => pro_model,
                _ => flash_model,
            };
            let mut profile = review_profile("reviewer", model, true);
            profile.llm_reasoning_effort =
                (matches!(variant, Variant::SinglePro)).then(|| "high".to_string());
            let worker_report = Worker::execute(
                &runtime,
                &profile,
                Task {
                    id: format!("{}-{}-{trial}", case.id, variant.as_str()),
                    source: "benchmark".into(),
                    tool: "agent".into(),
                    arguments: serde_json::json!({"task": case.prompt}),
                },
                None,
            )
            .await
            .map_err(|error| format!("{} trial failed: {error:#}", variant.as_str()))?;
            let outcome = format!("{:?}", worker_report.outcome);
            (
                worker_report.findings.clone(),
                worker_report.findings,
                outcome.clone(),
                outcome,
                None,
                false,
                count_disallowed_tool_calls(&worker_report.trace_events),
            )
        }
        Variant::LeadTwoWorkers => {
            let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
            let handoff = orchestrator
                .execute_review_pipeline(
                    &case.prompt,
                    review_profile("analysis-employee", flash_model, false),
                    review_profile("verification-employee", flash_model, true),
                    &format!("bench-{}-{trial}", case.id),
                )
                .await
                .map_err(|error| format!("lead_two_workers trial failed: {error:#}"))?;
            let lead_client = LlmClient {
                provider: pro_provider.clone(),
                model: pro_model.to_string(),
                profile_name: "benchmark-lead".into(),
                reasoning_effort: Some("high".into()),
                temperature: None,
                max_context_tokens: None,
            };
            let reports = [handoff.analyst.clone(), handoff.verifier.clone()];
            let lead_result = orchestrator
                .parent_review(&case.prompt, &reports, &[], &lead_client)
                .await;
            let lead_summary_succeeded = lead_result.is_ok();
            let lead = lead_result
                .map(|review| review.findings)
                .unwrap_or_else(|error| format!("Lead summary unavailable: {error}"));
            let analyst_outcome = format!("{:?}", handoff.analyst.outcome);
            let verifier_outcome = format!("{:?}", handoff.verifier.outcome);
            let recovery_candidate = !matches!(
                handoff.analyst.outcome,
                zhongshu_core::agent::RunOutcome::CompletedVerified
                    | zhongshu_core::agent::RunOutcome::CompletedUnverified
            ) && handoff.verifier.outcome
                == zhongshu_core::agent::RunOutcome::CompletedVerified
                && lead_summary_succeeded;
            let collaboration = CollaborationFacts {
                analyst_outcome,
                verifier_outcome,
                lead_summary_succeeded,
                acceptance_reasons: handoff.acceptance_reasons.clone(),
            };
            let disallowed_tool_calls = count_disallowed_tool_calls(&handoff.analyst.trace_events)
                .saturating_add(count_disallowed_tool_calls(&handoff.verifier.trace_events));
            (
                format!(
                    "分析员工：\n{}\n\n验证员工：\n{}\n\n中书：\n{}",
                    handoff.analyst.findings, handoff.verifier.findings, lead
                ),
                lead,
                format!("{:?}", handoff.status),
                match handoff.status {
                    WorkerExecutionStatus::Completed => "CompletedVerified".into(),
                    WorkerExecutionStatus::Submitted => "CompletedUnverified".into(),
                    _ => "Failed".into(),
                },
                Some(collaboration),
                recovery_candidate,
                disallowed_tool_calls,
            )
        }
    };
    let elapsed_ms = started.elapsed().as_millis();
    let mut provider_facts = Vec::new();
    for (trial_proxy, usage_meter) in [
        (&mut flash_proxy, &flash_usage),
        (&mut pro_proxy, &pro_usage),
    ] {
        trial_proxy.proxy.shutdown().await;
        let snapshot = settled_snapshot(&trial_proxy.proxy).await?;
        let provider_usage = usage_meter.snapshot();
        if provider_usage.failed_requests > 0 {
            return Err(format!(
                "trial {} {} provider group {} had {} failed provider request(s)",
                case.id,
                variant.as_str(),
                trial_proxy.role_group,
                provider_usage.failed_requests
            ));
        }
        if provider_usage.successful_responses > 0
            && (provider_usage.responses_missing_usage > 0 || provider_usage.total_tokens() == 0)
        {
            return Err(format!(
                "trial {} {} provider group {} returned {} successful response(s) but usable token accounting is missing; refusing a cost-blind benchmark",
                case.id,
                variant.as_str(),
                trial_proxy.role_group,
                provider_usage.successful_responses
            ));
        }
        if provider_usage.requests > 0 || !snapshot.conversation_ids.is_empty() {
            provider_facts.push(ProviderFacts {
                role_group: trial_proxy.role_group.clone(),
                upstream: trial_proxy.upstream.clone(),
                upstream_path: trial_proxy.upstream_path.clone(),
                model: trial_proxy.model.clone(),
                deeplossless_db: trial_proxy.db_path.display().to_string(),
                deeplossless: snapshot,
                provider_usage,
            });
        }
    }
    let score = score(
        case,
        &scored_report,
        &outcome,
        recovery_candidate,
        tool_policy_meter
            .violations
            .load(Ordering::Relaxed)
            .saturating_add(disallowed_tool_calls),
    );
    Ok(TrialResult {
        schema_version: 2,
        suite_id: suite_id.to_string(),
        case_id: case.id.clone(),
        variant,
        trial,
        flash_model: flash_model.to_string(),
        pro_model: pro_model.to_string(),
        elapsed_ms,
        live_limits: live_budget.limits,
        status,
        outcome,
        collaboration,
        score,
        provider_facts,
        report,
    })
}

fn count_disallowed_tool_calls(events: &[HarnessEvent]) -> u64 {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                HarnessEvent::ToolCall { tool_name, .. }
                    if tool_name != "read_file" && tool_name != "shell"
            )
        })
        .count() as u64
}

struct TrialProxy {
    role_group: String,
    upstream: String,
    upstream_path: String,
    model: String,
    db_path: PathBuf,
    proxy: DeeplosslessProxy,
}

async fn start_trial_proxy(
    facts_dir: &Path,
    role_group: &str,
    upstream: &str,
    upstream_path: &str,
    api_key: &str,
    model: &str,
) -> Result<TrialProxy, String> {
    let db_path = facts_dir.join(role_group).join("lcm.db");
    if db_path.exists() {
        return Err(format!(
            "refusing to reuse benchmark fact store {}; choose a new --output",
            db_path.display()
        ));
    }
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let mut proxy = DeeplosslessProxy::new(DeeplosslessConfig {
        db_path: db_path.display().to_string(),
        api_key: api_key.to_string(),
        upstream: upstream.to_string(),
        upstream_path: upstream_path.to_string(),
        summarize_model: model.to_string(),
        proxy_port: 0,
    })
    .await
    .map_err(|error| format!("cannot build Deeplossless {role_group} proxy: {error}"))?;
    proxy
        .start(0)
        .await
        .map_err(|error| format!("cannot start Deeplossless {role_group} proxy: {error}"))?;
    Ok(TrialProxy {
        role_group: role_group.to_string(),
        upstream: upstream.to_string(),
        upstream_path: upstream_path.to_string(),
        model: model.to_string(),
        db_path,
        proxy,
    })
}

fn resolved_upstream_path<'a>(
    tier_path: Option<&'a str>,
    common_path: Option<&'a str>,
    upstream: &str,
) -> &'a str {
    tier_path.or(common_path).unwrap_or_else(|| {
        let after_scheme = upstream
            .split_once("://")
            .map_or(upstream, |(_, rest)| rest);
        let has_base_path = after_scheme
            .split_once('/')
            .is_some_and(|(_, path)| !path.trim_matches('/').is_empty());
        if has_base_path {
            "/chat/completions"
        } else {
            "/v1/chat/completions"
        }
    })
}

async fn settled_snapshot(
    proxy: &DeeplosslessProxy,
) -> Result<DeeplosslessBenchmarkSnapshot, String> {
    let mut snapshot = proxy
        .benchmark_snapshot()
        .map_err(|error| format!("cannot read Deeplossless benchmark facts: {error}"))?;
    for _ in 0..20 {
        if snapshot.total_tokens() > 0 || snapshot.execution_ids.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        snapshot = proxy
            .benchmark_snapshot()
            .map_err(|error| format!("cannot refresh Deeplossless benchmark facts: {error}"))?;
    }
    Ok(snapshot)
}

fn review_profile(name: &str, model: &str, verification: bool) -> AgentProfile {
    let mut tools = vec!["read_file".into()];
    if verification {
        tools.push("shell".into());
    }
    let budget = AgentBudget {
        max_steps: if verification { 8 } else { 6 },
        max_tool_calls: if verification { 10 } else { 7 },
        per_tool_limit: 3,
        token_limit: if verification { 32_000 } else { 24_000 },
        llm_timeout: std::time::Duration::from_secs(120),
        tool_timeout: std::time::Duration::from_secs(60),
    };
    let mut profile = AgentProfile::new(
        name,
        "只读审查当前工作目录中的微型 Rust fixture；目标代码固定在 ./src/lib.rs，manifest 固定在 ./Cargo.toml。直接读取这两个文件，不要搜索或定位其他目录。不得修改文件，也不得在 /tmp 或其他位置创建临时测试或脚本。需要验证时，shell 工具只接受 {\"command\":\"cargo test\"}，且最多调用一次；不要添加重定向、cd、--manifest-path、cwd 或其他参数。并发缺陷可用源码级具体交错论证，无法动态复现时应明确说明。获得证据后立即给出最终报告，不要以裸 observation 结束。",
        tools,
        budget,
    );
    profile.llm_model = Some(model.to_string());
    profile.verification_policy = if verification {
        zhongshu_core::agent::VerificationPolicy::Required
    } else {
        zhongshu_core::agent::VerificationPolicy::NotRequired
    };
    profile
}

fn score(
    case: &Case,
    report: &str,
    outcome: &str,
    recovery_candidate: bool,
    tool_policy_violations: u64,
) -> TrialScore {
    let haystack = report.to_lowercase();
    let expected_found = case
        .expected_keywords
        .iter()
        .filter(|keyword| haystack.contains(&keyword.to_lowercase()))
        .count();
    let forbidden_found: Vec<String> = case
        .forbidden_keywords
        .iter()
        .filter(|keyword| haystack.contains(&keyword.to_lowercase()))
        .cloned()
        .collect();
    let expected_total = case.expected_keywords.len();
    let keyword_recall = expected_found as f64 / expected_total as f64;
    let terminal_completed = outcome == "CompletedVerified";
    let trimmed = report.trim();
    let report_is_final =
        !(trimmed.starts_with("<observation") && trimmed.ends_with("</observation>"));
    let content_rubric_passed =
        expected_found == expected_total && forbidden_found.is_empty() && report_is_final;
    let tool_policy_compliant = tool_policy_violations == 0;
    TrialScore {
        expected_found,
        expected_total,
        forbidden_found: forbidden_found.clone(),
        keyword_recall,
        terminal_completed,
        report_is_final,
        content_rubric_passed,
        recovery_succeeded: recovery_candidate && content_rubric_passed,
        tool_policy_compliant,
        tool_policy_violations,
        passed: content_rubric_passed && terminal_completed && tool_policy_compliant,
    }
}

fn copy_dir(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target)
        .map_err(|error| format!("cannot create {}: {error}", target.display()))?;
    for entry in fs::read_dir(source)
        .map_err(|error| format!("cannot read {}: {error}", source.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.file_name() == "target" || entry.file_name() == ".git" {
            continue;
        }
        let destination = target.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &destination)?;
        } else {
            fs::copy(entry.path(), &destination)
                .map_err(|error| format!("cannot copy {}: {error}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn write_trial(output: &Path, result: &TrialResult) -> Result<(), String> {
    let path = output.join(format!(
        "{}-{}-{}.json",
        result.case_id,
        result.variant.as_str(),
        result.trial
    ));
    fs::write(
        &path,
        serde_json::to_vec_pretty(result).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

#[allow(clippy::too_many_arguments)]
fn write_abort_report(
    output: &Path,
    suite_id: &str,
    case: &Case,
    variant: Variant,
    trial: usize,
    error: &str,
    live_budget: LiveBudgetSnapshot,
    completed_trials: usize,
) -> Result<PathBuf, String> {
    let path = output.join("aborted.json");
    let report = AbortReport {
        schema_version: 1,
        suite_id: suite_id.to_string(),
        case_id: case.id.clone(),
        variant,
        trial,
        error: error.to_string(),
        live_budget,
        completed_trials,
        evidence_note: "Request admissions and tokens are invocation-wide counters. Token totals include only provider responses that returned usable usage metadata; an interrupted or usage-missing response can cost more than recorded here.",
    };
    fs::write(
        &path,
        serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(path)
}

fn write_summary(output: &Path, suite_id: &str, results: &[TrialResult]) -> Result<(), String> {
    let passed = results.iter().filter(|result| result.score.passed).count();
    let content_rubric_passed = results
        .iter()
        .filter(|result| result.score.content_rubric_passed)
        .count();
    let terminal_completed = results
        .iter()
        .filter(|result| result.score.terminal_completed)
        .count();
    let recovery_succeeded = results
        .iter()
        .filter(|result| result.score.recovery_succeeded)
        .count();
    let tool_policy_compliant = results
        .iter()
        .filter(|result| result.score.tool_policy_compliant)
        .count();
    let total_tokens: u64 = results
        .iter()
        .flat_map(|result| &result.provider_facts)
        .map(|facts| facts.provider_usage.total_tokens())
        .sum();
    let summary = serde_json::json!({
        "schema_version": 2,
        "suite_id": suite_id,
        "evidence_level": "runtime_verified_with_real_provider",
        "trials": results.len(),
        "passed": passed,
        "failed": results.len().saturating_sub(passed),
        "content_rubric_passed": content_rubric_passed,
        "terminal_completed": terminal_completed,
        "recovery_succeeded": recovery_succeeded,
        "tool_policy_compliant": tool_policy_compliant,
        "provider_reported_total_tokens": total_tokens,
        "note": "passed is strict: content rubric, terminal completion, and tool-policy compliance must all pass. recovery_succeeded separately records a Lead producing a rubric-valid synthesis after the analyst failed and the verifier independently completed with fresh evidence. The keyword content rubric is smoke-grade, not a blinded quality judgment. Provider response usage is the cost source."
    });
    let path = output.join("summary.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&summary).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

struct CurrentDirGuard(PathBuf);

impl CurrentDirGuard {
    fn enter(path: &Path) -> Result<Self, String> {
        let previous = env::current_dir().map_err(|error| error.to_string())?;
        env::set_current_dir(path)
            .map_err(|error| format!("cannot enter {}: {error}", path.display()))?;
        Ok(Self(previous))
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        let _ = env::set_current_dir(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zhongshu_core::agent::llm::{FinalChoice, Message, Usage};

    struct UsageProvider;

    struct MissingUsageProvider;

    fn test_live_budget() -> Arc<LiveBudgetGuard> {
        Arc::new(LiveBudgetGuard::new(LiveLimits {
            max_requests: 10,
            max_tokens: 1_000,
            max_elapsed_secs: 30,
        }))
    }

    #[async_trait::async_trait]
    impl LlmProvider for UsageProvider {
        async fn chat(
            &self,
            _request: ChatCompletionRequest,
        ) -> anyhow::Result<ChatCompletionResponse> {
            Ok(ChatCompletionResponse {
                choices: vec![FinalChoice {
                    message: Message::assistant("done"),
                    finish_reason: Some("stop".into()),
                }],
                usage: Some(Usage {
                    prompt_tokens: 11,
                    completion_tokens: 7,
                    total_tokens: 18,
                }),
            })
        }

        async fn stream_chat(
            &self,
            _request: ChatCompletionRequest,
            _on_event: Box<dyn FnMut(StreamEvent) + Send>,
        ) -> anyhow::Result<()> {
            anyhow::bail!("unused in test")
        }

        fn model_name(&self) -> &str {
            "usage-model"
        }

        fn change_model(&self, _model: &str) -> Arc<dyn LlmProvider> {
            Arc::new(Self)
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for MissingUsageProvider {
        async fn chat(
            &self,
            _request: ChatCompletionRequest,
        ) -> anyhow::Result<ChatCompletionResponse> {
            Ok(ChatCompletionResponse {
                choices: vec![FinalChoice {
                    message: Message::assistant("done"),
                    finish_reason: Some("stop".into()),
                }],
                usage: None,
            })
        }

        async fn stream_chat(
            &self,
            _request: ChatCompletionRequest,
            _on_event: Box<dyn FnMut(StreamEvent) + Send>,
        ) -> anyhow::Result<()> {
            anyhow::bail!("unused in test")
        }

        fn model_name(&self) -> &str {
            "missing-usage-model"
        }

        fn change_model(&self, _model: &str) -> Arc<dyn LlmProvider> {
            Arc::new(Self)
        }
    }

    #[test]
    fn live_mode_requires_explicit_models() {
        let args =
            parse_args(&["--suite".into(), "suite.json".into(), "--live".into()]).expect("parse");
        assert!(args.live);
        assert!(args.flash_model.is_none());
        assert!(args.pro_model.is_none());
    }

    #[tokio::test]
    async fn metered_provider_keeps_usage_across_model_changes() {
        let meter = Arc::new(ProviderUsageMeter::default());
        let provider: Arc<dyn LlmProvider> = Arc::new(MeteredProvider {
            inner: Arc::new(UsageProvider),
            meter: meter.clone(),
            live_budget: test_live_budget(),
        });
        provider
            .change_model("other")
            .chat(ChatCompletionRequest {
                model: "other".into(),
                messages: vec![Message::user("review")],
                tools: None,
                tool_choice: None,
                stream: false,
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
            })
            .await
            .expect("chat");
        let usage = meter.snapshot();
        assert_eq!(usage.requests, 1);
        assert_eq!(usage.successful_responses, 1);
        assert_eq!(usage.responses_missing_usage, 0);
        assert_eq!(usage.total_tokens(), 18);
    }

    #[tokio::test]
    async fn metered_provider_rejects_first_response_without_usage() {
        let meter = Arc::new(ProviderUsageMeter::default());
        let provider = MeteredProvider {
            inner: Arc::new(MissingUsageProvider),
            meter: meter.clone(),
            live_budget: test_live_budget(),
        };
        let error = provider
            .chat(ChatCompletionRequest {
                model: "missing-usage-model".into(),
                messages: vec![Message::user("review")],
                tools: None,
                tool_choice: None,
                stream: false,
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
            })
            .await
            .expect_err("missing usage must stop immediately");
        assert!(error.to_string().contains("omitted usage"));
        let usage = meter.snapshot();
        assert_eq!(usage.requests, 1);
        assert_eq!(usage.responses_missing_usage, 1);
    }

    #[test]
    fn parses_distinct_worker_and_lead_providers() {
        let args = parse_args(&[
            "--suite".into(),
            "suite.json".into(),
            "--flash-upstream".into(),
            "https://workers.example".into(),
            "--flash-api-key-env".into(),
            "WORKER_KEY".into(),
            "--pro-upstream".into(),
            "https://lead.example".into(),
            "--pro-api-key-env".into(),
            "LEAD_KEY".into(),
            "--flash-upstream-path".into(),
            "/chat/completions".into(),
        ])
        .expect("parse");
        assert_eq!(
            args.flash_upstream.as_deref(),
            Some("https://workers.example")
        );
        assert_eq!(args.pro_upstream.as_deref(), Some("https://lead.example"));
        assert_eq!(args.flash_api_key_env.as_deref(), Some("WORKER_KEY"));
        assert_eq!(args.pro_api_key_env.as_deref(), Some("LEAD_KEY"));
        assert_eq!(
            args.flash_upstream_path.as_deref(),
            Some("/chat/completions")
        );
    }

    #[test]
    fn infers_chat_path_for_host_and_versioned_base_urls() {
        assert_eq!(
            resolved_upstream_path(None, None, "https://api.deepseek.com"),
            "/v1/chat/completions"
        );
        assert_eq!(
            resolved_upstream_path(
                None,
                None,
                "https://ark.cn-beijing.volces.com/api/coding/v3"
            ),
            "/chat/completions"
        );
        assert_eq!(
            resolved_upstream_path(Some("/custom/chat"), None, "https://provider.example/base"),
            "/custom/chat"
        );
    }

    #[test]
    fn parses_single_trial_canary_filters() {
        let args = parse_args(&[
            "--suite".into(),
            "suite.json".into(),
            "--case".into(),
            "completion-admission-race".into(),
            "--variant".into(),
            "single_flash".into(),
            "--repeats".into(),
            "1".into(),
        ])
        .expect("parse");
        assert_eq!(
            args.case_filter.as_deref(),
            Some("completion-admission-race")
        );
        assert_eq!(args.variant_filter, Some(Variant::SingleFlash));
        assert_eq!(args.repeats, 1);
    }

    #[test]
    fn parses_live_cost_limits() {
        let args = parse_args(&[
            "--suite".into(),
            "suite.json".into(),
            "--max-requests".into(),
            "5".into(),
            "--max-tokens".into(),
            "12000".into(),
            "--max-elapsed-secs".into(),
            "90".into(),
        ])
        .expect("parse limits");

        assert_eq!(args.max_requests, 5);
        assert_eq!(args.max_tokens, 12_000);
        assert_eq!(args.max_elapsed_secs, 90);
    }

    #[test]
    fn live_matrix_is_rejected_before_provider_setup() {
        let args = parse_args(&[
            "--suite".into(),
            "suite.json".into(),
            "--live".into(),
            "--flash-model".into(),
            "flash".into(),
            "--pro-model".into(),
            "pro".into(),
        ])
        .expect("parse");

        let error = validate_live_invocation(&args).expect_err("matrix must remain disabled");
        assert!(error.contains("--case"));
        assert!(error.contains("--repeats 1"));
    }

    #[test]
    fn live_budget_rejects_requests_before_they_exceed_the_cap() {
        let budget = LiveBudgetGuard::new(LiveLimits {
            max_requests: 1,
            max_tokens: 100,
            max_elapsed_secs: 30,
        });

        assert!(budget.admit_request().is_ok());
        let error = budget
            .admit_request()
            .expect_err("second request must stop");
        assert!(error.to_string().contains("request limit 1"));
        assert_eq!(budget.admitted_requests.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn live_budget_stops_after_provider_reports_token_overrun() {
        let budget = LiveBudgetGuard::new(LiveLimits {
            max_requests: 2,
            max_tokens: 10,
            max_elapsed_secs: 30,
        });

        let error = budget.record_tokens(18).expect_err("token limit must stop");
        assert!(error.to_string().contains("token limit 10 exceeded"));
        let next_error = budget
            .admit_request()
            .expect_err("no provider call may run after token overrun");
        assert!(next_error.to_string().contains("token limit 10 reached"));
    }

    #[test]
    fn live_budget_clamps_each_request_to_reported_remaining_tokens() {
        let budget = LiveBudgetGuard::new(LiveLimits {
            max_requests: 2,
            max_tokens: 100,
            max_elapsed_secs: 30,
        });
        budget
            .provider_tokens
            .store(75, std::sync::atomic::Ordering::Relaxed);
        let mut request = ChatCompletionRequest {
            model: "model".into(),
            messages: vec![Message::user("review")],
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: None,
            max_tokens: Some(80),
            reasoning_effort: None,
        };

        budget.clamp_request_tokens(&mut request);

        assert_eq!(request.max_tokens, Some(25));
    }

    #[tokio::test]
    async fn metered_provider_rejects_streaming_before_provider_call() {
        let meter = Arc::new(ProviderUsageMeter::default());
        let provider = MeteredProvider {
            inner: Arc::new(UsageProvider),
            meter: meter.clone(),
            live_budget: test_live_budget(),
        };
        let error = provider
            .stream_chat(
                ChatCompletionRequest {
                    model: "model".into(),
                    messages: vec![Message::user("review")],
                    tools: None,
                    tool_choice: None,
                    stream: true,
                    temperature: None,
                    max_tokens: None,
                    reasoning_effort: None,
                },
                Box::new(|_| {}),
            )
            .await
            .expect_err("streaming must be rejected before provider use");

        assert!(error.to_string().contains("streaming is disabled"));
        assert_eq!(meter.snapshot().requests, 0);
    }

    #[test]
    fn live_budget_rejects_request_after_elapsed_deadline() {
        let budget = LiveBudgetGuard {
            limits: LiveLimits {
                max_requests: 2,
                max_tokens: 100,
                max_elapsed_secs: 1,
            },
            started: Instant::now() - Duration::from_secs(2),
            admitted_requests: AtomicU64::new(0),
            provider_tokens: AtomicU64::new(0),
        };

        let error = budget
            .admit_request()
            .expect_err("elapsed deadline must stop");
        assert!(error.to_string().contains("elapsed-time limit 1s"));
        assert_eq!(budget.admitted_requests.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn abort_report_preserves_cost_counters_and_uncertainty() {
        let output = TempDir::new().expect("temporary output");
        let case = Case {
            id: "case".into(),
            title: "case".into(),
            fixture: "fixture".into(),
            prompt: "review".into(),
            expected_keywords: vec!["issue".into()],
            forbidden_keywords: vec![],
        };
        let path = write_abort_report(
            output.path(),
            "suite",
            &case,
            Variant::SingleFlash,
            1,
            "request limit reached",
            LiveBudgetSnapshot {
                limits: LiveLimits {
                    max_requests: 2,
                    max_tokens: 100,
                    max_elapsed_secs: 30,
                },
                admitted_requests: 2,
                provider_reported_tokens: 75,
                elapsed_ms: 12,
            },
            0,
        )
        .expect("write abort report");

        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).expect("read abort report"))
                .expect("parse abort report");
        assert_eq!(report["live_budget"]["admitted_requests"], 2);
        assert_eq!(report["live_budget"]["provider_reported_tokens"], 75);
        assert!(report["evidence_note"]
            .as_str()
            .expect("evidence note")
            .contains("can cost more"));
    }

    #[test]
    fn rejects_unknown_variant() {
        let error = parse_args(&[
            "--suite".into(),
            "suite.json".into(),
            "--variant".into(),
            "cheap".into(),
        ])
        .expect_err("unknown variant must fail");
        assert!(error.contains("unknown benchmark variant"));
    }

    #[test]
    fn review_profiles_keep_small_benchmark_budgets() {
        let analyst_profile = review_profile("analyst", "flash", false);
        let verifier_profile = review_profile("verifier", "flash", true);
        assert!(!analyst_profile
            .tool_names
            .iter()
            .any(|tool| tool == "search_files"));
        assert_eq!(analyst_profile.tool_names, vec!["read_file"]);
        assert_eq!(verifier_profile.tool_names, vec!["read_file", "shell"]);
        assert!(analyst_profile.system_prompt.contains("./src/lib.rs"));
        let analyst = analyst_profile.to_worker_budget();
        let verifier = verifier_profile.to_worker_budget();
        assert_eq!(analyst.max_steps, 6);
        assert_eq!(analyst.max_tool_calls, 7);
        assert_eq!(verifier.max_steps, 8);
        assert_eq!(verifier.max_tool_calls, 10);
    }

    #[test]
    fn score_rejects_keywords_when_outcome_is_not_verified() {
        let case = Case {
            id: "case".into(),
            title: "case".into(),
            fixture: "fixture".into(),
            prompt: "review".into(),
            expected_keywords: vec!["Submitted".into(), "verification".into()],
            forbidden_keywords: vec!["safe to merge".into()],
        };
        let result = score(
            &case,
            "Submitted because verification evidence is missing",
            "CompletedUnverified",
            false,
            0,
        );
        assert!(!result.passed);
        assert!(result.content_rubric_passed);
        assert_eq!(result.keyword_recall, 1.0);
        assert!(!result.terminal_completed);
    }

    #[test]
    fn score_rejects_bare_observation_even_when_verified() {
        let case = Case {
            id: "case".into(),
            title: "case".into(),
            fixture: "fixture".into(),
            prompt: "review".into(),
            expected_keywords: vec!["busy".into()],
            forbidden_keywords: vec![],
        };
        let result = score(
            &case,
            "<observation tool=\"read_file\">busy</observation>",
            "CompletedVerified",
            false,
            0,
        );
        assert!(!result.passed);
        assert!(!result.content_rubric_passed);
        assert!(!result.report_is_final);
    }

    #[test]
    fn score_records_recovery_without_hiding_terminal_failure() {
        let case = Case {
            id: "case".into(),
            title: "case".into(),
            fixture: "fixture".into(),
            prompt: "review".into(),
            expected_keywords: vec!["race".into()],
            forbidden_keywords: vec![],
        };
        let result = score(&case, "The verifier confirmed the race.", "Failed", true, 0);

        assert!(result.content_rubric_passed);
        assert!(result.recovery_succeeded);
        assert!(!result.terminal_completed);
        assert!(!result.passed);
    }

    #[tokio::test]
    async fn benchmark_test_tool_rejects_arbitrary_shell_without_executing_it() {
        let policy_meter = Arc::new(ToolPolicyMeter::default());
        let tool = BenchmarkTestTool {
            policy_meter: policy_meter.clone(),
        };

        let output = tool
            .execute(&serde_json::json!({"command": "printf forbidden > /tmp/file"}))
            .await;

        assert_eq!(output.status, ToolStatus::Error);
        assert_eq!(policy_meter.violations.load(Ordering::Relaxed), 1);
        assert!(output
            .error
            .as_deref()
            .is_some_and(|error| error.contains("only the exact command")));
    }

    #[tokio::test]
    async fn benchmark_test_tool_rejects_cwd_override() {
        let policy_meter = Arc::new(ToolPolicyMeter::default());
        let tool = BenchmarkTestTool {
            policy_meter: policy_meter.clone(),
        };

        let output = tool
            .execute(&serde_json::json!({"command": "cargo test", "cwd": "/tmp"}))
            .await;

        assert_eq!(output.status, ToolStatus::Error);
        assert_eq!(policy_meter.violations.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn benchmark_test_tool_accepts_only_exact_cargo_test_shape() {
        assert!(is_allowed_benchmark_test_call(
            &serde_json::json!({"command": "cargo test"})
        ));
        assert!(!is_allowed_benchmark_test_call(
            &serde_json::json!({"command": "cargo test 2>&1"})
        ));
        assert!(!is_allowed_benchmark_test_call(
            &serde_json::json!({"command": "cargo test", "cwd": "."})
        ));
    }

    #[tokio::test]
    async fn benchmark_test_tool_rejects_a_second_test_run() {
        let policy_meter = Arc::new(ToolPolicyMeter::default());
        policy_meter.test_calls.store(1, Ordering::Relaxed);
        let tool = BenchmarkTestTool {
            policy_meter: policy_meter.clone(),
        };

        let output = tool
            .execute(&serde_json::json!({"command": "cargo test"}))
            .await;

        assert_eq!(output.status, ToolStatus::Error);
        assert_eq!(policy_meter.violations.load(Ordering::Relaxed), 1);
        assert!(output
            .error
            .as_deref()
            .is_some_and(|error| error.contains("at most once")));
    }

    #[test]
    fn benchmark_read_tool_accepts_only_exact_fixture_paths() {
        assert_eq!(
            allowed_benchmark_read_path(&serde_json::json!({"path": "./Cargo.toml"})),
            Some("./Cargo.toml")
        );
        assert_eq!(
            allowed_benchmark_read_path(&serde_json::json!({"path": "./src/lib.rs"})),
            Some("./src/lib.rs")
        );
        assert_eq!(
            allowed_benchmark_read_path(&serde_json::json!({"path": "/workspace/src/lib.rs"})),
            None
        );
        assert_eq!(
            allowed_benchmark_read_path(
                &serde_json::json!({"path": "./src/lib.rs", "extra": true})
            ),
            None
        );
    }

    #[tokio::test]
    async fn benchmark_read_tool_records_out_of_scope_attempt() {
        let policy_meter = Arc::new(ToolPolicyMeter::default());
        let tool = BenchmarkReadTool {
            policy_meter: policy_meter.clone(),
        };

        let output = tool
            .execute(&serde_json::json!({"path": "/etc/passwd"}))
            .await;

        assert_eq!(output.status, ToolStatus::Error);
        assert_eq!(policy_meter.violations.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn tool_policy_counts_calls_outside_the_profile_allowlist() {
        let events = vec![
            HarnessEvent::ToolCall {
                step: 1,
                tool_name: "read_file".into(),
                args_hash: "a".into(),
                success: true,
            },
            HarnessEvent::ToolCall {
                step: 2,
                tool_name: "write_file".into(),
                args_hash: "b".into(),
                success: false,
            },
        ];

        assert_eq!(count_disallowed_tool_calls(&events), 1);
    }
}
