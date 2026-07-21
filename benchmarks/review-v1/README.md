# Review benchmark v1

This suite compares `single_pro`, `lead_two_workers`, and `single_flash` on
read-only review tasks. Fixture source is visible to the agent; the expected
keyword rubric is runner-only metadata and is never copied into a trial
workspace.

Validate the suite without API calls:

```bash
cargo xtask benchmark \
  --suite benchmarks/review-v1/suite.json \
  --dry-run \
  --repeats 3
```

Real-provider execution is deliberately opt-in and requires explicit model
names:

```bash
cargo xtask benchmark \
  --suite benchmarks/review-v1/suite.json \
  --live \
  --repeats 1 \
  --case completion-admission-race \
  --variant single_flash \
  --flash-model YOUR_FLASH_MODEL \
  --pro-model YOUR_PRO_MODEL \
  --output .roadmap/project-review-2026-07-17/benchmarks/canary-001
```

Live mode is currently canary-only: it requires `--case`, `--variant`, and
`--repeats 1`. A single invocation is capped across Lead and workers at 12
provider requests, 40,000 provider-reported tokens, and 180 seconds. Override
these downward or, after an explicit cost review, upward with
`--max-requests`, `--max-tokens`, and `--max-elapsed-secs`.
Each non-streaming request also has its output-token parameter clamped to the
remaining provider-reported budget. Streaming is rejected before a provider
call because this runner cannot account its usage safely.

Do not begin with the full matrix. Run one cheap canary first:

```bash
cargo xtask benchmark \
  --suite benchmarks/review-v1/suite.json \
  --live --repeats 1 \
  --case completion-admission-race \
  --variant single_flash \
  --flash-model YOUR_FLASH_MODEL \
  --pro-model YOUR_PRO_MODEL \
  --max-requests 8 \
  --max-tokens 25000 \
  --max-elapsed-secs 120 \
  --output .roadmap/project-review-2026-07-17/benchmarks/canary-001
```

Valid variants are `single_flash`, `single_pro`, and `lead_two_workers`.
Omitting `--case` or `--variant` expands that dimension only for dry-run;
live matrix execution is rejected by the safety gate.

Lead and workers may use different OpenAI-compatible providers:

```bash
cargo xtask benchmark \
  --suite benchmarks/review-v1/suite.json \
  --live --repeats 1 \
  --case completion-admission-race \
  --variant lead_two_workers \
  --flash-upstream https://worker-provider.example \
  --flash-api-key-env WORKER_API_KEY \
  --flash-model WORKER_MODEL \
  --pro-upstream https://lead-provider.example \
  --pro-api-key-env LEAD_API_KEY \
  --pro-model LEAD_MODEL \
  --output .roadmap/project-review-2026-07-17/benchmarks/run-002
```

`--upstream` and `--api-key-env` remain shorthand when both model tiers use
the same provider. The runner infers `/v1/chat/completions` for a host-only
upstream and `/chat/completions` when the upstream already contains a base
path such as `/api/coding/v3`. Ambiguous providers can override this with
`--flash-upstream-path`, `--pro-upstream-path`, or the common
`--upstream-path`.

Each trial gets an isolated Deeplossless `lcm.db`. Zhongshu stores aggregate
usage and replay anchors in the trial result; when providers differ, Lead and
workers receive separate fact stores. Raw model/execution facts remain in those
databases. Provider response `usage` is the benchmark cost source because some
Deeplossless/provider combinations do not currently populate the database token
counters. The runner refuses to continue after a successful response with
missing or zero provider usage. Benchmark workers cannot use the system-wide
`search_files`/`locate` or `self_test` tools. Its `read_file` capability accepts
only the exact fixture paths `./Cargo.toml` and `./src/lib.rs`. The exposed
`shell` is replaced by a benchmark-only tool that accepts exactly one
`cargo test` call; it cannot interpret shell syntax, change cwd, or create
helper scripts.

If a live trial aborts, the runner writes `aborted.json` in the selected output
directory before returning an error. It records the invocation-wide request
admissions, provider-reported tokens, elapsed time, failed case/variant, and the
error. Its token count is intentionally qualified: an interrupted response or a
response without usable `usage` metadata may have incurred additional provider
cost that the runner cannot measure.

Result schema v2 reports four dimensions separately:

- `content_rubric_passed`: the smoke-grade keyword rubric passed. For
  `lead_two_workers`, only the Lead synthesis is scored, not worker observations.
- `terminal_completed`: the production orchestration reached verified completion.
- `recovery_succeeded`: after an analyst failure, the verifier produced fresh
  evidence and the Lead produced a rubric-valid synthesis.
- `tool_policy_compliant`: the model made no rejected shell attempt or duplicate
  test run.

`passed` remains strict and requires the content rubric, terminal completion,
and tool-policy compliance. Recovery is reported without rewriting a real
`WorkerFailed` status as success. The keyword rubric is not a substitute for
blinded quality review.

`lead_two_workers` specifically benchmarks the dedicated `review_pipeline`.
It is not the general multi-employee organization benchmark. Role staffing is
covered offline in `../organization-v1` until its execution and handoff gates
are qualified without provider calls.
