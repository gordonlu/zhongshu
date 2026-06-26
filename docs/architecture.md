# 中叔 Architecture

## Overview

中叔 (zhongshu) is a desktop AI assistant overlay: a persistent, always-on companion that lives in the system tray, listens via hotkey, and renders AI responses in a floating WebView window. It is optimized for DeepSeek V4 (flash/pro) with deeplossless DAG compression, context budgets, equipment-based skill composition, and background autonomy (scheduled tasks, memory evaluation, suggestions, auto-evolution).

### Guiding Principles

1.  **One assistant, one window** — a single persistent GTK/wry window; show/hide, never destroy.
2.  **LLM-optimized for DeepSeek** — model routing (flash for chat, pro for deep work), `reasoning_effort`, 500k–1M context windows.
3.  **Self-evolution** — equipment system lets the LLM install/update its own skills and tools at runtime.
4.  **Background autonomy** — heartbeat-driven rule engine, scheduled tasks, auto-memory consolidation, auto-evolution.

---

## Crate Structure

```
zhongshu/                         # workspace root
├── Cargo.toml                    # workspace definition, members
├── zhongshu-core/                # core library — agent loop, tools, context, DB, equipment
├── zhongshu-orb/                 # desktop application — GTK/wry/winit windowing, tray, overlay
├── zhongshu-cli/                 # CLI client (minimal, for testing)
└── zhongshu-message-core/        # streaming message parsing, block types
```

### `zhongshu-core` (library crate)

The core agent runtime. No windowing, no platform-specific code.

| Module | Path | Purpose |
|--------|------|---------|
| `agent` | `src/agent/` | ReAct loop, runtime, worker dispatcher, profile, attention, LLM abstraction, model router, LLM registry |
| `core` | `src/core/` | Database, context packing (ContextPack), runbook, goal/task/suggestion/observation/memory subsystems, event log, artifact repo, scheduler |
| `tool` | `src/tool/` | All tool implementations (browser, search, shell, filesystem, webfetch, automation, CDP browser, self_test) |
| `equipment` | `src/equipment/` | Skill/equipment registry, manifest, permission, observer, built-in defaults |
| `event` | `src/event/` | EventBus, typed events (agent state, tool call, permission, equipment, runbook, workflow) |
| `integration` | `src/integration/` | Deeplossless proxy integration (session management, compress) |
| `authority` | `src/authority/` | Permission gate (allow/deny/pending/always-ask) |
| `desktop` | `src/desktop/` | OS-level desktop integration (clipboard, notification, system info) |
| `digest` | `src/digest/` | Attention digest builder |
| `heartbeat` | `src/heartbeat/` | Periodic tick for background checks |
| `rule` | `src/rule/` | Rule engine — event patterns trigger tasks |
| `source` | `src/source/` | Information sources (battery, disk, timer) |
| `task` | `src/task/` | Task scheduler, triggers (reminder, file watch) |

### `zhongshu-orb` (binary crate)

The desktop application. Wires `zhongshu-core` into a visible assistant.

| Module | Path | Purpose |
|--------|------|---------|
| `main.rs` | `src/main.rs` | Entry point — init tracing, proxy, DB, equipment, controller, background services, event loop |
| `app.rs` | `src/app.rs` | `AgentController` — state machine (idle/thinking/responding/error), mode switching, context assembly, IPC dispatch |
| `handler.rs` | `src/handler.rs` | `ZhongshuApp` — winit event handler, tray, overlay, resize, auth overlay |
| `overlay.rs` | `src/overlay.rs` | Overlay IPC methods (chat, debug, equipment, auth, settings) |
| `agent.rs` | `src/agent.rs` | `AgentProfile` wrapper with `to_state_block()`, `AgentMemory` |
| `config.rs` | `src/config.rs` | Config file load/save, LLM config structs, system prompt assembly |
| `services.rs` | `src/services.rs` | Background tokio tasks (scheduler, memory eval, suggestion, workflow, task executor, auto-evolution) |
| `render.rs` | `src/render.rs` | Procedural orb rendering (12-layer Siri-inspired, wgpu) |
| `indicator.rs` | `src/indicator.rs` | Tray icon state machine (idle/active/thinking/error) |
| `hotkey.rs` | `src/hotkey.rs` | Global hotkey registration |

### `zhongshu-cli` (binary crate)

Minimal CLI for quick testing — sends a prompt, streams reply to stdout.

### `zhongshu-message-core` (library crate)

Streaming message parser. Splits SSE stream into structured blocks (text, tool_call, tool_result, control).

---

## Startup Sequence

```
main()
├── tracing_subscriber::fmt()         # log level from ZHONGSHU_LOG env
├── preflight_checks()                # verify EventBus and channel basics
├── config::load()                    # read/save config.json
├── tokio runtime (multi-thread, 4 workers)
├── DeeplosslessProxy::new() + start()  # local proxy on port 610XX
├── EventBus (4096 capacity)
├── response_tx/rx channel (mpsc)
├── authority::init(AuthorityGate)    # permission gate singleton
├── AttentionDispatcher               # desktop notifications for attention events
├── EquipmentRegistry                 # install defaults, load skills, build system prompt
├── Database (core.db)                # migrate tables
├── Infrastructure objects            # ObservationStore, SuggestionEngine, MemoryPolicy, etc.
├── OpenAiProvider                    # primary LLM client (pointed at deeplossless proxy)
├── Background services               # spawned as tokio tasks:
│   ├── scheduler loop
│   ├── memory evaluation (30s)
│   ├── suggestion analysis (3600s)
│   ├── event observation feed
│   ├── event workflow
│   ├── task executor
│   ├── llm suggestion engine (900s)
│   ├── compensation
│   └── auto-evolution (3600s)
├── AgentController                   # core UI-facing agent state machine
├── AgentInbox                        # background message processing thread
├── TaskScheduler                     # reminders + file watches
├── AgentRuntime (shared, for workers)
├── Worker profiles                   # loaded from profiles/ dir, dispatched per task
├── RuleEngine                        # event → task rules (heartbeat-check)
├── Heartbeat                         # periodic tick source
├── DigestBuilder                     # daily attention digest
├── EventLogger                       # JSONL persistence + replay
├── ZhongshuApp (winit::application)  # create window, tray, start event loop
└── EventLoop::run_app()
```

---

## Data Flow: User Input → Response

```
User types in overlay textarea
        │
        ▼
window.handleIpc(JSON.parse(...))     # JS→wry IPC
        │
        ▼
handler receives IPC string
        │
        ▼
match ipc.command
  "chat" => AgentController::handle_chat()
  "mode_change" => set_mode(), refresh_skill_prompts()
  "toggle_equipment" => toggle equipment, refresh_skill_prompts()
  "show_equipment" => return equipment list
  "delete_chat" => delete_chat_history()
  ...
        │
        ▼
AgentController::handle_chat()
├── AgentProfile::to_state_block()       # personality, budget → XML state
├── AgentMemory::to_state_block()        # long-term memory → XML state
├── ContextPackBuilder::new(stable_system)
│   ├── with_state(profile_block + memory_block)
│   ├── with_evidence(...)                # scored evidence from external sources
│   ├── with_recent_history(history)      # RecentUnit chain
│   ├── with_new_input(user_message)      # final user message
│   ├── select_mode(mode)                # assistant vs coding budget
│   └── build()                          # returns ContextPack (or ContextTooLong error)
│
├── mode == "coding"
│   ? AgentBudget::coding_default()      # 200/400/200, 1M ctx, 600s/300s timeouts
│   : AgentBudget::assistant_default()   # 80/160/40, 500k ctx, 240s/120s timeouts
│
├── run_agent_with_context(
│       runtime,
│       context_pack.into_llm_messages(),
│       AgentBudget,
│       AgentCallbacks { on_text, on_tool, ... }
│   )
│       │
│       ▼
│   run_agent() loop (ReAct):
│   ┌──────────────────────────────────────────────────┐
│   │ 1. build_request() → create chat completion body │
│   │    (model routing, reasoning_effort, budget)     │
│   │ 2. provider.stream() → SSE stream                │
│   │    (with tokio::timeout per budget)              │
│   │ 3. stream_messages() → parse stream              │
│   │    (ControlTokenFilter persists across deltas)   │
│   │ 4. On tool_call:                                 │
│   │    a. check_budget()                             │
│   │    b. tool_registry.dispatch()                   │
│   │       (with tokio::timeout per budget)           │
│   │    c. add result to messages                     │
│   │ 5. On final_answer:                              │
│   │    a. strip <final_answer> tags                  │
│   │    b. return LoopResult::Finished(text)          │
│   │ 6. On max_steps exceeded: return LoopResult::MaxSteps
│   └──────────────────────────────────────────────────┘
│
├── Decode: strip <observation>/<lcm_context> from output
├── Save to agent.json (long_term_memory, conversation)
├── Save observation/event to EventBus
├── Notify EquipmentObserver (record_user_message)
├── Send ResponseEvent::Delta → response_tx
│       │
│       ▼
├── Overlay::push_chat()              # updates chat.html DOM
│   ├── stream message to message list
│   └── filter by assistant_id
│
└── Done → Overlay::show_chat_complete() # update model name, stop streaming indicator
```

---

## Core Component Details

### 1. Agent Loop (`agent/loop_.rs`)

The ReAct loop is the core inference engine:

```
run_agent(context, budget, callbacks)
  ├── model_router.select_role()       # flash for simple, pro for complex
  ├── llm.stream()                     # SSE POST to deeplossless proxy
  ├── stream_messages()                # parse SSE → Message variants
  │   ├── TextDelta → on_text callback
  │   ├── ToolCallBatch → execute tools
  │   └── FinalAnswer → return
  ├── on ToolCallBatch:
  │   ├── check_budget(budget)         # max steps, max tool calls
  │   ├── tool_registry.call(name, args) → Result
  │   └── append ToolResult to messages
  ├── on FinalAnswer:
  │   └── strip tags, return LoopResult::Finished
  └── on error/timeout:
      └── return LoopResult::Error or LoopResult::Timeout
```

**Streaming details:**
- `ControlTokenFilter` has a `pending: String` buffer that persists across delta boundaries — critical for split control tokens (e.g., `<final_ans` in one chunk, `wer>\n...` in the next).
- Detected control prefixes: `final_answer`, `observation`, `lcm_context`, `tool_call`.
- TextDelta is not skipped when `finish_reason = "stop"` — the final content is important.

**Budget enforcement:**
- `tokio::time::timeout` wraps LLM streaming calls and tool execution calls.
- `check_budget()` checks `step_count < max_steps` and `tool_call_count < max_tool_calls`.
- `Timeout` stop reason returned if exceeded.

### 2. ContextPack (`core/context.rs`)

Structured context packing, 703 lines, zero dependency on `agent::llm` types.

**Architecture:**

```
ContextPackBuilder
├── stable_system: String              # base system + skills + safety (KV cache stable prefix)
├── state: Vec<StateBlock>             # agent profile, memory (instructional="false")
├── evidence: Vec<EvidenceBlock>       # scored external content
├── recent: Vec<RecentUnit>            # conversation history
├── input: Option<String>              # new user message
├── mode: ContextMode                  # token limit selection
│
├── build() → Result<ContextPack, ContextError>
│   ├── score + sort evidence (desc by relevance × confidence × source_weight)
│   ├── crop evidence to fit token budget
│   ├── crop history: preserve causal tails (ToolChain units atomic)
│   ├── assemble into ContextMessage::System (stable) + User (state + evidence + recent + input)
│   └── token budget check: stable + state + evidence + recent + input ≤ max tokens
│       └── input never trimmed → returns ContextTooLong if it overflows
│
└── ContextPack
    └── into_llm_messages() → Vec<agent::llm::Message>
        └── render_context() formats state/evidence as XML with escaping
```

**Key types:**

| Type | Purpose |
|------|---------|
| `ContextPackBuilder` | Builder with `with_state()`, `with_evidence()`, `with_recent_history()`, `into_llm_messages()` |
| `ContextPack` | Final assembled context, convertible to LLM messages |
| `ContextPackReport` | Debug info: token counts per section, crop decisions, unit stats |
| `StateBlock` | Profile/goals/memory → `<state instructional="false">` |
| `EvidenceBlock` | Scored external content (relevance × confidence × source_weight) |
| `RecentUnit` | History: `UserAssistant`, `ToolChain`, `Memo` |
| `ContextMessage` | Internal message representation (System/User/Assistant/Tool) |

**Cropping strategy:**
1. Evidence: descending score, keep highest.
2. History: `RecentUnit` chain — preserve tail (most recent), crop oldest from front. `ToolChain` units (tool_call → result → followup) are atomic — never split.
3. Input: never trimmed. If input alone + stable prefix exceeds budget, return `ContextTooLong`.

**Evidence scoring:**
```
score = relevance × confidence × source_weight
```
- Sorted descending.
- `score_threshold` filters low-value evidence.
- `recency` deferred to V2.

### 3. Tool System (`tool/`)

`ToolRegistry` maps tool name → `Box<dyn Tool>`.

**Tool interface:**
```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;           // JSON Schema
    async fn call(&self, args: Value) -> ToolResult;
}
```

**All tools:**

| Tool | File | Description |
|------|------|-------------|
| `WebSearchTool` | `search.rs` | DuckDuckGo search, human_delay, concurrency limit |
| `BrowserTool` | `browser.rs` | Headless browser (reqwest), HTML → markdown |
| `BrowserAutomationTool` | `browser_automation.rs` | Chrome CDP: 16 actions (open/snapshot/click/type/scroll/press/...) |
| `WebFetchTool` | `webfetch.rs` | Direct URL fetch + HTML → markdown |
| `ShellTool` | `shell.rs` | Command execution (sandboxed) |
| `GrepTool` | `fs.rs` | Grep delegate to shell `rg` |
| `GlobTool` | `fs.rs` | Glob delegate to `find` |
| `EditTool` | `fs.rs` | read + write + replace file operations |
| `SearchFilesTool` | `search_files.rs` | Combined file search |
| `AutomationTool` | `automation.rs` | Desktop automation (ydotool/wtype) |
| `ScreenshotTool` | `screenshot.rs` | **Disabled** |
| `SelfTestTool` | `self_test.rs` | Integration test runner |
| `MemoryTool` | `memory.rs` | CRUD on long-term memory |
| `GoalTool` | `core/goal/tool.rs` | Goal CRUD + status |
| `TaskTool` | `core/task/tool.rs` | Task CRUD + execution |
| `SuggestionTool` | `core/suggestion/tool.rs` | LLM suggestion retrieval |
| `MemoryQueryTool` | `core/memory/tool.rs` | Policy-based memory query |

**Concurrency & safety:**
- `human_delay()`: 1–3s random delay before HTTP tools.
- `acquire_http_permit()`: semaphore (max 3) for HTTP concurrency.
- `sanitize_web_content()`: strips zero-width chars, null bytes, detects garbled content.
- `build_browser_client()`: realistic Chrome 149 headers, cookies, gzip/brotli support.

### 4. Equipment System (`equipment/`)

A skill-extension system that lets the LLM modify its own capabilities at runtime.

```
EquipmentRegistry
├── install_defaults()                 # built-in skills
├── install(manifest, code)            # install from LLM proposal
├── uninstall(id)                      # remove skill
├── toggle(id, enabled)               # enable/disable
├── skill_prompts() → Vec<(id, String)>  # active skill system prompts
└── list() → Vec<EquipmentManifest>    # all equipment metadata

EquipmentObserver
├── record_user_message(text)          # observe user behavior
├── on_event(event)                   # observe system events
└── spawn_auto_evaluation()           # periodic trigger

spawn_auto_evolution()                 # background service (3600s interval)
├── observer → evaluation → LLM proposal
├── parse_proposal_response()          # extract install/uninstall/toggle ops
├── registry.apply(proposal)           # install/uninstall/toggle
└── refresh_skill_prompts()            # rebuild system prompt
```

### 5. LLM Registry (`agent/llm_registry.rs`)

Multi-profile LLM configuration:

```
LlmRegistry
├── profiles: HashMap<String, LlmProfile>  # named model configs
├── roles: HashMap<String, String>          # role → profile name mapping
├── fallback: Vec<String>                   # fallback chain
│
├── client_for_role("primary") → LlmClient  # resolve role → profile → client
├── client_for_role("worker") → LlmClient
└── client_for_role("suggestion") → LlmClient
```

Each `LlmProfile` specifies: model name, base URL, API key, max tokens, reasoning effort.

`LlmProvider` trait has `change_model()` for model switching within a provider. `OpenAiProvider` overrides to create a new provider instance with the target model.

### 6. Budget System (`agent/loop_.rs`)

Mode-driven step budgets:

| Budget | Mode | Max Steps | Max Tool Calls | Per-tool Limit | Token Limit | LLM Timeout | Tool Timeout |
|--------|------|-----------|----------------|----------------|-------------|-------------|--------------|
| `assistant_default` | assistant | 80 | 160 | 40 | 500k | 240s | 120s |
| `coding_default` | coding | 200 | 400 | 200 | 1M | 600s | 300s |

`AgentBudgetProfile` (config) holds only `token_limit` with `#[serde(default)]` for backward compatibility. The step/timeout defaults are hardcoded by mode.

### 7. Event System (`event/`)

Typed, channel-based EventBus:

```
EventBus (tokio::sync::broadcast)
├── AgentEvent: StateChanged, Thinking, Idle, Error
├── ToolEvent: ToolCalled, ToolResult, PermissionRequired
├── EquipmentEvent: SkillInstalled, SkillToggled
├── RunbookEvent: Step, Complete
└── WorkflowEvent: TaskCreated, TaskCompleted

Subscribers:
├── EventLogger → JSONL file
├── ObservationStore → DB
├── Workflow engine → event-driven tasks
├── AttentionDispatcher → desktop notifications
├── EquipmentObserver → auto-evolution
└── Compensation → reward feedback
```

### 8. Background Services (`orb/src/services.rs`)

All spawned as `tokio::spawn` tasks at startup:

| Service | Interval | Purpose |
|---------|----------|---------|
| `spawn_scheduler` | continuous | Drives core scheduler events |
| `spawn_memory_evaluation` | 30s | Evaluate memory candidates → consolidate |
| `spawn_suggestion_analysis` | 3600s | Update suggestion scores |
| `spawn_event_observation_feed` | continuous | Feed events → ObservationStore |
| `spawn_event_workflow` | continuous | Event-driven task orchestration |
| `spawn_task_executor` | continuous | Execute queued tasks (background agent calls) |
| `spawn_llm_suggestion_engine` | 900s | LLM-generated suggestions |
| `spawn_compensation` | continuous | Reward/feedback processing |
| `spawn_auto_evolution` | 3600s | Equipment self-evolution via LLM |

---

## Orb Layer (Desktop Application)

### Window Stack

```
winit::window (always-on-top, transparent)
├── GTK socket (Wayland considerations)
├── wry WebView (WebKitGTK)
│   └── chat.html (overlay UI)
│       ├── message list (streaming markdown)
│       ├── equipment panel
│       ├── debug overlay
│       └── auth permission bar (sticky bottom)
└── tray icon (breathing: 1Hz idle, 10Hz active)
```

### State Machine (`app.rs: AgentController`)

```
States: Idle ↔ Thinking → Responding → Idle
                ↘ Error → Idle

Idle:       waiting for user input
Thinking:   LLM streaming (tool calls in progress)
Responding: final answer streaming to UI
Error:      displayed in overlay, auto-returns to Idle
```

**`handle_chat()` pipeline (simplified):**
1. Set state → Thinking
2. Build context: profile + memory + history + input via `ContextPackBuilder`
3. Select mode-driven `AgentBudget`
4. `run_agent_with_context()` with streaming callbacks
5. On each `on_text`: decode + send `ResponseEvent::Delta` → overlay
6. On each `on_tool`: send tool badge IPC
7. On complete: save to memory, save conversation, notify observers
8. Set state → Idle

### Overlay IPC (`handler.rs → overlay.rs`)

All UI communication goes through `JSON.parse(...)` safe encoding:

```
wry IPC: window.handleIpc(JSON.parse({...}))
  ├── push_chat(data)         → append message to chat
  ├── show_chat_complete()    → finalize message, show model
  ├── push_debug(event)       → debug overlay (cap 100, show last 50)
  ├── show_model(model_str)   → update model name in header
  ├── show_auth(request)      → auth permission bar
  ├── show_notification(n)    → desktop notification
  ├── set_mode(mode)          → badge update + window resize
  └── sync_equipment(list)    → equipment panel
```

### GTK + Window Management

- `gtk::init()` must be on the same thread as `gtk::main()`. A `GTK_TX` lazy static creates a dedicated thread.
- Window is created once, shown/hidden, never destroyed.
- Mode switching triggers resize: coding 860×900, assistant 520×800.
- Context menu (Windows only): "New Conversation" / "Quit".

---

## Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **Deeplossless proxy in-process** | Avoids external process management. Proxy starts before agent, routes all LLM calls through `localhost:610XX`. |
| **ContextPack as core type, not agent type** | `core/context.rs` uses its own `ContextMessage`/`RecentUnit` to avoid circular dependency on `agent::llm`. `into_llm_messages()` is the sole bridge. |
| **Stable prefix = stable_system only** | State (goals/memory/profile) changes — only the base system + skills + safety rules are stable for KV cache reuse. |
| **Input never auto-trimmed** | If `stable_system + input` exceeds max tokens, `build()` returns `ContextTooLong`. Input is never silently cropped. |
| **ToolChain units never broken** | Tool calls and their results form causal pairs. Cropped as complete units only. |
| **Evidence scoring = product of 3 factors** | `relevance × confidence × source_weight`. Simple, composable. Recency deferred to V2. |
| **Auth overlay gated behind flag** | `pending_auth_notified` prevents the auth bar from re-appearing every GTK tick (~60fps) after user interaction. |
| **History loads across ALL conversations** | No conversation filter on history load — restarts don't fragment context. `delete_chat_history` scoped by session_id only. |
| **ControlTokenFilter persistent across deltas** | `pending` buffer is not reset per delta, so split control tokens (e.g., `<final_ans` + `wer>`) are handled. |
| **Mode switching via config + IPC** | Mode is persisted in config, flows through full IPC pipeline: button → handler → config save → `set_mode()` → `refresh_skill_prompts()`. |
| **Auto-evolution via LLM proposals** | EquipmentObserver collects behavior → LLM proposes skill changes → registry applies. 3600s interval. |
| **Worker LLM via trait method** | `LlmProvider::change_model()` default returns clone. `OpenAiProvider` creates new provider with different model. |
| **Concurrency limit at tool level** | Semaphore per HTTP request (max 3), not per client. |
| **Press action uses serde_json** | Prevents JS injection via template literal interpolation in CDP `evaluate()`. |

---

## Database Schema

### `core.db` (SQLite)

| Table | Purpose | Created By |
|-------|---------|------------|
| `goals` | Goal tracking | core migration |
| `goal_steps` | Goal step tracking | core migration |
| `tasks` | Task queue | core migration |
| `task_logs` | Task execution history | core migration |
| `observations` | Event observations | core migration |
| `memory_candidates` | Memory candidates for evaluation | core migration |
| `memory_policy` | Memory retention policy | core migration |
| `suggestions` | LLM suggestions | core migration |
| `suggestion_logs` | Suggestion execution history | core migration |
| `events` | Event log | core migration |
| `runbooks` | Runbook store | core migration |
| `runbook_steps` | Runbook step store | core migration |
| `artifacts` | Artifact repository | core migration |
| `schedules` | Scheduler entries | core migration |

### `lcm.db` (external, managed by Deeplossless)

| Table | Purpose |
|-------|---------|
| `messages` | Chat messages (all conversations) |
| `dag_nodes` | DAG-compressed context nodes |
| `conversations` | Conversation metadata |

### `agent.json` (file)

| Field | Purpose |
|-------|---------|
| `conversation` | Recent conversation pairs (for reload) |
| `long_term_memory` | LLM-managed memory array (2000 char limit) |
| `system_prompt` | Code-only (`#[serde(skip_serializing)]`) — always reset to default on load |

---

## Deployment

- **Linux**: GTK3 + wry (WebKitGTK). Single binary.
- **Windows**: Planned — winit + wry (WebView2) + softbuffer orb. `gtk::init()` replaced with `windows::Application`.
- **CI**: GitHub Actions — `test-core` (parallel), `test-core-auth` (`--test-threads=1` for singleton tests).
