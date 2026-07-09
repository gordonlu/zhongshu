## Objective
- Implement interruption handling: when the user sends a message mid-execution, the system pauses the current run, classifies intent, injects recovery context (partial response + completed steps), and re-enters the agent loop naturally — no stream token leakage, no lost progress, no mechanical "new session" behavior.

## Important Details
- Work is on **main** (merged the task/closure branch earlier); all new interruption-handling commits build on main
- Design approved: `docs/superpowers/specs/2026-07-09-interruption-handling-design.md`
- Plan written: `docs/superpowers/plans/2026-07-09-interruption-handling.md`
- Execution mode: subagent-driven (one subagent per task, review gate per task)
- Key design decisions:
  - Streaming user interjection → hard interrupt (cancel LLM stream immediately, best-effort async cancel, run_id-filter stale deltas)
  - ReadOnly tools: wait for completion if fast (<1s), can cancel if long (>2s); Write/System/External/Irreversible tools: force pause, require re-confirmation
  - Intent classification: keyword-based, no LLM dependency
  - CancellationToken (tokio-util) for interruption signaling in agent loop

## Commit History (Task 1-10)
```
2e42293 - Task 9 baseline
317e56f - Task 1: SideEffect enum + ToolSpec.side_effect field + inference
db0ccf1 - Task 2: RunEvent enum + Event::Run variant + run_id on ResponseEvent + ToolEvent.Interrupted
0d01b26 - Task 3: intent.rs — InterruptionIntent + intent_classify() with 7 intent types + 7 tests
c39c067 - Task 4: run.rs — RunController (state machine, interrupt flow, determine_action, build_recovery_prompt) + 7 tests
94c500a - Task 5: CancellationToken in agent loop, run_id on AgentCallbacks
1991d42 - Task 6: handler.rs — run_id filter, RunEvent UI toasts
24a2f24 - Task 7: Tool execution interruption — side_effect-aware cancellation
8be5dfb - Task 8: Approval loop — cancel on interruption
3115aee - Task 9: App wiring — RunController in AgentController, interrupt on busy, callbacks with run_id
d38e986 - Task 10: Suppress dead_code warnings
1cbe1b3 - Fixes from code review (critical): sync active_run_id from RunController, consume InterruptionAction, track stop_reason + overall_success, keep run_id on resume
f8e3c6c - Fixes from code review (important): rapid-interrupt guard, RunState pattern matching, Resuming event emission
```

## Work State
### Completed (Tasks 1-10)
- **Task 1:** SideEffect enum + ToolSpec.side_effect field + inference function (commit `317e56f`)
- **Task 2:** RunEvent enum + Event::Run variant + run_id on ResponseEvent + ToolEvent.Interrupted (commit `db0ccf1`)
- **Task 3:** intent.rs — InterruptionIntent + intent_classify() with 7 intent types + 7 tests (commit `0d01b26`)
- **Task 4:** run.rs — RunController state machine (Idle→Running→Interrupted→Resuming→Finished), determine_action, build_recovery_prompt (commit `c39c067`)
- **Task 5:** CancellationToken on run_agent/run_agent_with_context/stream_step/sync_step, tokio::select! wrapping LLM call, run_id on AgentCallbacks (commit `94c500a`)
- **Task 6:** handler.rs — active_run_id filtering in reduce_responses, RunEvent match arms for Interrupted/Resuming/Cancelled (commit `1991d42`)
- **Task 7:** Tool execution — cancel check before execute, side-effect-aware behavior (commit `24a2f24`)
- **Task 8:** Approval loop — tokio::select! with cancel_token.cancelled(), deny pending + system message on interruption (commit `8be5dfb`)
- **Task 9:** AgentController gets run_controller: Arc<RunController>, interrupt() on busy, recovery re-run with CancellationToken, run_id through callbacks (commit `3115aee`)
- **Task 10:** Integration compilation, zero warnings (commit `d38e986`)
- **Code review fixes:** Critical bugs (sync active_run_id, consume InterruptionAction, track stop_reason/overall_success, keep run_id on resume); Important fixes (rapid-interrupt guard, RunState pattern matching, Resuming event emission) (commits `1cbe1b3`, `f8e3c6c`)

### Assessment
**Ready to merge?** Yes — all critical and important review issues resolved. 528 core tests + 49 orb tests pass (2 pre-existing env config failures). Zero warnings on `cargo check`.

### Blocked
- (none)

### Pending
- **Task 11:** Task worker integration (bonus, low priority) — not started

## Relevant Files
- `zhongshu-core/src/agent/run.rs` — RunController, RunState, InterruptionCtx, InterruptionAction, determine_action, interrupt, build_recovery_prompt, begin_resume, finish_run
- `zhongshu-core/src/agent/intent.rs` — intent_classify, InterruptionIntent
- `zhongshu-core/src/agent/loop_.rs` — CancellationToken, run_id in AgentCallbacks, tool interruption, approval cancellation
- `zhongshu-core/src/event/mod.rs` — RunEvent, ResponseEvent.run_id, ToolEvent.Interrupted
- `zhongshu-core/src/tool/spec.rs` — SideEffect enum
- `zhongshu-orb/src/app.rs` — AgentController with RunController, recovery path with InterruptionAction routing
- `zhongshu-orb/src/handler.rs` — ZhongshuApp with run_id filtering, RunEvent UI toasts
- `docs/superpowers/specs/2026-07-09-interruption-handling-design.md`
- `docs/superpowers/plans/2026-07-09-interruption-handling.md`
