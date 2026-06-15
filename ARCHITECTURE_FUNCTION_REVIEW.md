# Zhongshu 架构与主要功能 Review

日期：2026-06-15

## 范围与假设

本报告基于当前仓库源码、Cargo workspace 配置、`ROADMAP.md`、核心入口文件和测试结果整理。当前终端对部分历史中文注释/文档显示为乱码，因此报告不逐字引用这些乱码文本，只基于可确认的模块名、类型、函数、依赖和调用链判断。

本报告重点是“架构和主要功能 review”，不是逐行代码审查。发现的明显风险会单独列出。

## 总体结论

项目是一个 Rust workspace，目标形态是长期运行的桌面 Agent Runtime。当前代码已经形成四层结构：

1. `zhongshu-core`：核心运行时、Agent loop、工具系统、事件总线、持久化域模型、后台调度与安全授权。
2. `zhongshu-message-core`：消息解析与流式渲染辅助，负责把 LLM 输出从原始字符串转成块结构，并过滤 agent 协议控制 token。
3. `zhongshu-cli`：命令行入口，复用核心 Agent Runtime 和默认工具。
4. `zhongshu-orb`：桌面入口，使用 `winit`/`wry`/`gtk` 构建 orb、overlay、WebView IPC、任务面板、授权 UI 和后台事件流。

架构方向清晰：核心逻辑放在 `zhongshu-core`，UI 和桌面生命周期放在 `zhongshu-orb`。不过桌面入口仍然承担了较多编排职责，后台任务、事件订阅、LLM 执行、数据库 store 初始化和 UI action reducer 都集中在 `zhongshu-orb/src/main.rs`，后续维护风险偏高。

## Workspace 结构

### `zhongshu-core`

核心库，无 UI 依赖。主要模块：

- `agent/`：LLM provider、`AgentRuntime`、ReAct loop、worker/profile/report、attention manager。
- `core/`：SQLite 持久化域模型，包含 goals、tasks、observations、suggestions、artifacts、memory、event log、scheduler。
- `event/`：`EventBus`、`ResponseEvent`、事件日志 JSONL replay。
- `tool/`：shell、filesystem、web search、browser、webfetch、screenshot、desktop automation、search files、memory、system info。
- `authority/`：命令风险分类、授权 gate、TTL approval cache、pending auth singleton。
- `task/`：轻量后台 task queue、trigger、scheduler。
- `rule/` / `source/` / `heartbeat/` / `digest/`：事件源、规则引擎、心跳和 digest。
- `integration/deeplossless.rs`：deeplossless 代理和历史会话读取/删除。

### `zhongshu-message-core`

面向 UI 渲染的消息层：

- `parser.rs`：解析 markdown-ish 文本为 `BlockTree`。
- `block.rs`：定义 `MessageBlock` 和 `BlockTree`。
- `streaming.rs`：`StreamingMessage` 增量解析，`ControlTokenFilter` 清理 `<final_answer>`、`<observation>` 等控制 token。

### `zhongshu-cli`

简单 REPL：

- 从 `DEEPSEEK_API_KEY` 读取 key。
- 默认 model 为 `deepseek-v4-flash`。
- 注册默认工具，并额外注册 web search/browser/screenshot/desktop automation。
- 每轮创建 `AgentRuntime` 并调用 `run_agent`。

### `zhongshu-orb`

桌面应用：

- `main.rs`：winit 应用主循环、事件 reducer、响应流 reducer、后台任务初始化、事件源/规则/调度器/worker/LLM provider wiring。
- `app.rs`：`AgentController`、`AgentInbox`、`TaskWorkerDispatcher`。
- `overlay.rs`：WebView overlay、IPC action、设置/授权/任务 UI 接口。
- `config.rs`：配置加载、迁移、保存、默认 system prompt/personality/background/scheduler/deeplossless 设置。
- `agent.rs`：本地 agent profile/todo/goal/long-term memory JSON。
- `indicator.rs` / `hotkey.rs`：桌面 orb、托盘/热键。

## 核心运行流

### 交互式 Agent 流

1. UI 或 CLI 收到用户输入。
2. 构建 messages：system prompt、可选 profile/memory context、历史消息、用户输入。
3. 创建或复用 `AgentRuntime`，其中包含 LLM provider、tool registry、model、budget。
4. `run_agent` 执行 ReAct loop：
   - 调用 LLM。
   - 若返回 tool calls，按工具名从 `ToolRegistry` 执行。
   - 工具输出统一渲染成 `<observation tool="..." status="...">`。
   - 支持 streaming callback、工具开始/完成 callback。
   - 有 max steps、max tool calls、token estimate 和连续工具失败保护。
5. `zhongshu-orb` 将 `ResponseEvent` 推给 overlay，使用 `ControlTokenFilter` 去掉控制 token。
6. 完成后更新状态、历史、todo/goal completion，并回到 idle。

### 工具调用与授权

`ToolRegistry` 是工具系统核心。工具实现 `Tool` trait，暴露名称、描述、JSON schema 参数和 `execute`。

授权逻辑位于 `authority/`：

- shell 命令会被解析为 program、args、pipe、redirect、chaining、targets。
- 风险等级为 `Safe`、`Moderate`、`Dangerous`、`Blocked`。
- 危险命令要求用户授权，授权按 `(tool, program)` 缓存 TTL。
- 阻断类命令直接拒绝，例如系统路径破坏、部分提权破坏操作、明显危险磁盘操作。
- screenshot/browser/automation 这类非 shell 工具也可以被 `check_tool` 标记为需要授权。

### 持久化核心域模型

`core/db.rs` 使用 SQLite，并开启 WAL 与 foreign keys。当前 schema 包含：

- `observations`
- `suggestions`
- `goals`
- `tasks`
- `task_steps`
- `task_runs`
- `artifacts`
- `task_artifacts`
- `memory_candidates`
- `memories`
- `events`

对应 repository/store 封装在 `core/*` 下。模型集中在 `core/models.rs`。

### Goal / Task / Scheduler 流

已有两套相关机制：

1. `core/goal` 和 `core/task`：持久化 goal/task，并通过 `GoalTool`、`TaskTool` 暴露给 agent。
2. `core/scheduler`：扫描 active goals，为 one-shot/recurring/ongoing goal 创建 task，并可发布 `TaskEvent::Triggered`。

桌面端还启动 `zhongshu_core::task::TaskScheduler`，注册 reminder/file watch trigger，并通过 rule engine 把事件转成后台任务。

当前 `TaskPlanner` 明确是启发式实现：按标题关键词生成固定步骤，不是 LLM planner。

### Observation / Suggestion / Memory 流

已有实现：

- EventBus 中的 Agent/Tool 事件会写入 `ObservationStore`。
- `SuggestionEngine::analyze` 会基于近期 observation 做简单模式分析。
- orb 里另有定时 LLM suggestion 分析，每 30 分钟读取近期 observation，要求 LLM 返回 JSON 数组后写入 suggestions。
- `SuggestionTool` 可 list/accept/reject suggestion，accept 后通过事件流创建 goal/task。
- `MemoryCandidateStore` 记录候选记忆。
- `MemoryPolicy::evaluate` 每 10 分钟晋升 confidence >= 0.7 的候选为 memories。
- `MemoryPolicy::search` 当前是 SQLite `LIKE` 关键词搜索，`embedding` 字段存在但未实际填充或检索。

### 桌面 UI / IPC 流

orb 启动后：

- 创建 orb indicator 和 overlay WebView。
- overlay 通过 IPC 提交输入、审批授权、修改设置、新建会话、停止当前 agent、查看/完成/取消任务。
- `ZhongshuApp::drain` 同时处理 EventBus、ResponseEvent、pending auth 和 overlay actions。
- UI response 通过 `ResponseEvent::MessageStarted/Delta/Completed` 增量推送。
- 历史会话从 deeplossless 的 `lcm.db` 加载，展示最近 20 轮，旧历史懒加载。

## 主要功能状态

| 功能 | 当前状态 | 依据 |
| --- | --- | --- |
| CLI 对话 | 已实现 | `zhongshu-cli/src/main.rs` 使用 `run_agent` 和工具注册 |
| OpenAI-compatible provider | 已实现 | `agent/llm/openai.rs` 支持 chat 与 stream_chat |
| ReAct loop | 已实现 | `agent/loop_.rs` 支持 tool calls、budget、失败保护 |
| Tool registry | 已实现 | `tool/mod.rs` |
| Shell/FS/System tools | 已实现 | `tool/shell.rs`、`tool/fs.rs`、`tool/system_info.rs` |
| Web/browser/webfetch/search_files/automation/screenshot | 已实现或有条件启用 | orb 注册大多数工具，screenshot 在 orb 中注释为禁用 |
| 授权 gate | 已实现且测试覆盖较多 | `authority/mod.rs` |
| SQLite core DB | 已实现 | `core/db.rs`、repository/store |
| Goal/Task tool | 已实现 | `core/goal/tool.rs`、`core/task/tool.rs` |
| Task planner | 初版启发式 | `core/task/planner.rs` 明确写着 future LLM planner |
| Task executor | 桌面端已有事件驱动执行链 | `zhongshu-orb/src/main.rs` 中监听 `TaskEvent::Triggered` |
| Observation pipeline | 已实现基础链路 | EventBus -> ObservationStore |
| Suggestion engine | 初版实现 | pattern analyzer + orb 中定时 LLM analyzer |
| Memory pipeline | 初版实现 | candidate -> confidence policy -> memory，LIKE search |
| EventBus | 已实现 | broadcast channel，允许 lag/drop |
| Event log replay | 已实现 | JSONL append/replay，跳过部分 stale events |
| Message rendering core | 已实现且测试覆盖较多 | parser/streaming tests |
| Desktop overlay | 已实现但强依赖桌面 GTK/WebView 环境 | `zhongshu-orb` |
| deeplossless proxy/history | 已实现 | `integration/deeplossless.rs` |

## 架构优点

1. 核心和 UI 边界基本正确：`zhongshu-core` 不依赖 orb UI，CLI 和 desktop 都复用同一套 agent/tool/runtime。
2. 事件模型比较统一：Agent、Tool、Task、Memory、Goal、Suggestion、Authority、Attention、Source 都挂到 `Event` enum 下。
3. 工具接口简单清楚：schema、execute、observation 输出都集中在 `Tool` trait 及 `ToolOutput`。
4. 安全授权有独立模块，并且覆盖了 shell 解析、敏感路径、系统路径、提权和缓存授权等测试。
5. 数据层收敛到 SQLite repository/store，主要业务对象没有散落为临时 JSON。
6. 消息流对 agent 协议 token 做了隔离，避免 `<observation>`、`<final_answer>` 泄漏到 UI。

## 主要风险与差距

### 1. `zhongshu-orb/src/main.rs` 编排职责过重

`main.rs` 同时负责应用生命周期、UI reducer、配置响应、数据库 store 初始化、后台定时任务、LLM task executor、suggestion analyzer、source manager、rule engine、worker dispatcher、event logger。这会让后续改动难以局部验证。

建议后续按职责拆分为：

- runtime/bootstrap
- event wiring
- task execution service
- suggestion service
- memory service
- UI action reducer

### 2. Roadmap 中部分能力已建模但仍是初版

当前已存在 planner、memory embedding 字段、suggestion LLM analyzer、task executor 等结构，但能力还不是最终形态：

- `TaskPlanner` 是关键词启发式，不是 LLM planner。
- memory 没有 embedding 写入和向量检索。
- suggestion 的 LLM JSON 解析没有结构化容错流程，非 JSON 会直接落为空数组。
- task step 目前会生成并保存，但 executor 是把 steps 拼成 prompt 后一次 LLM 调用，并没有真正逐 step 状态推进。

### 3. 事件可靠性是“广播/可丢”的设计

`EventBus` 基于 `tokio::broadcast`，lag 时会丢事件。这对 UI 状态通知可以接受，但对“必须发生”的业务动作要谨慎。目前部分持久化和 workflow 依赖 EventBus 订阅触发，例如 suggestion accepted -> goal/task、task triggered -> executor。

如果某些业务语义要求 exactly-once 或 at-least-once，后续应使用持久化队列或数据库状态扫描补偿，而不是只依赖 broadcast。

### 4. 全局授权状态是 singleton，复杂并发下需要更严格边界

`authority` 使用全局 `OnceLock<Mutex<...>>` 保存 gate 和 pending request。当前简单桌面交互可工作，但多个 agent/worker 并发时，pending request 只有一个槽位，可能被后来的请求覆盖。

如果多 worker 或多会话并发成为常态，pending auth 应带 request id，并由 UI 针对 request id approve/deny。

### 5. 桌面 crate 的跨平台构建依赖需要明确

在当前 Windows 环境中，`cargo test --workspace` 因 `zhongshu-orb` 的 GTK 依赖需要 `pkg-config`/GLib/GDK 元数据而失败。说明桌面构建环境要求还需要文档化或通过 target-specific dependency/feature gating 降低默认 workspace 测试门槛。

## 验证结果

已执行：

```powershell
cargo test -p zhongshu-core -p zhongshu-message-core -p zhongshu-cli
```

结果：

- `zhongshu-core`：132 个测试通过。
- `zhongshu-core/tests/smoke.rs`：4 个测试通过。
- `zhongshu-message-core`：35 个测试通过。
- `zhongshu-cli`：0 个测试，通过。

也尝试执行：

```powershell
cargo test --workspace
```

结果：失败，失败点不在业务测试，而是在构建 `zhongshu-orb` 的 GTK 相关 sys crates 时找不到 `pkg-config`，包括 `gobject-2.0`、`glib-2.0`、`gio-2.0`、`pango`、`gdk-3.0`、`atk` 等。当前环境无法验证 `zhongshu-orb` 的完整编译和测试。

## 建议优先级

1. 先补构建文档或 CI profile：明确 Windows/Linux 下 orb 所需系统依赖，或让默认 `cargo test --workspace` 不被桌面 GTK 环境阻断。
2. 拆分 `zhongshu-orb/src/main.rs` 的后台服务初始化和 reducer，降低单文件耦合。
3. 为 EventBus 驱动的关键 workflow 增加数据库状态补偿，避免 broadcast lag/drop 导致任务丢失。
4. 将 authorization pending request 从单槽全局状态升级为 request-id 模型。
5. 明确 Phase 4 能力边界：LLM planner、逐 step executor、memory embedding/vector search、suggestion-to-goal 自动化应分别有可测试的最小闭环。

