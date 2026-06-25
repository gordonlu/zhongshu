# 项目 Diff Review：6baecc6..HEAD

生成日期：2026-06-25

Review 范围：

```text
6baecc6..HEAD
```

涉及提交：

```text
52f6f00 fix: equipment IPC backend, Worker LLM profile, mode filtering, page_errors addEventListener, network_start guard
9646423 fix: browser_session restored, action_risk wired, Runbook DDL in migration, LlmRegistry instantiated, press injection fixed
4de206f fix: remove broken browser_session, escape press action JS injection
8d1c0bb Phase 9/10: equipment management + debug panels (full UI)
74fa987 Phase 6D: Runbook artifact; Phase 7: Worker LLM profile selection; Phase 8: mode switching (full UI→config pipeline); Phase 9/10: overlay methods
fe992d8 Phase 6B/C: risk classification, network capture, page errors; Phase 8: mode toggle UI
fda00ed Phase 6C: CDP screenshot action; Phase 7: full LlmRegistry config integration
a6c7c20 Phase 7: LlmRegistry + multi-profile config structure
```

Review 重点：

- 多 LLM / worker profile 模型选择是否真实生效。
- 浏览器自动化新增动作是否可用、安全风险分类是否合理。
- Runbook 是否形成任务级闭环。
- 装备管理 UI 是否影响实际 system prompt。
- 编译和基础验证情况。

## 总体结论

这组 diff 搭了不少结构：

- `LlmRegistry` 和多 profile 配置结构。
- worker profile 的 LLM 字段。
- Runbook schema / store。
- 浏览器自动化新增动作。
- 装备管理 UI。
- 模式切换 UI 和 skill prompt 简单过滤。

但多个关键功能还没有闭环：

- 多 LLM registry 创建后没有进入 runtime。
- worker 模型选择只改 `runtime.model`，没有切换 provider 内部 model。
- 浏览器自动化的 `scroll` 和 `wait_for_selector` 有可复现逻辑错误。
- `eval` 风险等级过低。
- 装备启停不刷新 system prompt，也不持久化。
- Runbook 只有 schema/store，没有写入调用。

`cargo check -q` 通过，但 warning 也印证了部分功能未接入。

## Findings

### P1：多 LLM registry 创建后未使用

位置：

- `zhongshu-orb/src/main.rs:269`

现状：

```rust
let llm_registry = cfg.llm.to_registry();
```

问题：

- `llm_registry` 创建后没有传入 controller、worker runtime 或后台服务。
- 主 Agent 仍使用：

```rust
let provider = OpenAiProvider::new(&ak, &cfg.llm.model).with_base_url(base_url);
```

- worker runtime 也仍复用同一个 `provider.clone()`。
- memory / suggestion / task executor / auto-evolve 后台服务也仍使用旧 provider。
- `cargo check` 报 `unused variable: llm_registry`。

影响：

- `llm.profiles` / `llm.roles` 配置目前不会影响实际运行。
- “多 LLM 配置”看起来已经有结构，但用户配置后不会生效。

建议：

- 引入 `RuntimeFactory` 或把 `LlmRegistry` 注入 `AgentController` 和 worker dispatcher。
- 主 Agent 从 `role=primary` 取 `LlmClient`。
- worker 从 `worker.<profile.name>` -> `worker.default` -> default fallback。
- 后台服务按 role 取模型，例如：
  - `background.memory`
  - `background.suggestion`
  - `background.auto_evolve`
  - `task.planner`
  - `browser.planner`

验收：

- 配置不同 role/profile 后，实际请求发到不同 model/base_url。
- warning 消失。

---

### P1：Worker profile 模型选择不会真正生效

位置：

- `zhongshu-core/src/agent/worker.rs:88`
- `zhongshu-core/src/agent/llm/openai.rs`

现状：

```rust
AgentRuntime {
    provider: runtime.provider.clone(),
    model: profile
        .llm_model
        .clone()
        .unwrap_or_else(|| runtime.model.clone()),
    reasoning_effort: profile.llm_reasoning_effort.clone(),
}
```

问题：

- worker 只改了 `AgentRuntime.model`。
- 但 `OpenAiProvider::build_body` / `build_stream_body` 会用 provider 内部 `self.model` 覆盖 request model。
- worker 仍复用 `runtime.provider.clone()`，所以 provider 内部 model 没变。

影响：

- profile 里设置 `llm_model` 后，实际请求仍可能使用主 runtime 的 provider model。
- 这会让 worker 专用模型、编码模型、浏览器 planner 模型等能力失效。

建议：

- 如果 provider 是 `OpenAiProvider`，使用 `provider.with_model(profile_model)` 构造新 provider。
- 更通用方案：让 `AgentRuntime` 持有 `LlmClient`，由 `LlmClient::chat/stream_chat` 统一注入 model。
- 避免同时存在 `runtime.model` 和 provider 内部 model 两个权威来源。

验收：

- 写测试确认 worker profile model 会出现在真实 request body 中。

---

### P1：browser_automation.scroll 会抛 JS exception

位置：

- `zhongshu-core/src/tool/browser_automation.rs:97`

现状：

```rust
"scroll" => eval_js(&json!({
    "js": "window.scrollBy(0, arguments[0] || window.innerHeight/2)",
    "max_length": 100
})).await,
```

问题：

- `Runtime.evaluate` 执行的是顶层 JS 表达式。
- 这里的 `arguments[0]` 不存在。
- 调用 `scroll` 会抛 `ReferenceError: arguments is not defined`。

影响：

- 浏览器自动化的基础滚动动作不可用。
- 页面下半部分元素无法可靠进入视口，会影响填表、调试和 DOM snapshot 后续动作。

建议：

- 从参数读取 `y`，用 `serde_json::to_string` 注入数值：

```rust
let y = args["y"].as_i64().unwrap_or(0);
let js = format!("window.scrollBy(0, {})", y_or_default);
```

- 或实现独立 `scroll(args)` 函数，支持：
  - `x`
  - `y`
  - `selector`
  - `block=center`

验收：

- 单测覆盖 JS 字符串生成。
- 手动或集成测试确认页面滚动位置变化。

---

### P1：wait_for_selector 永远可能返回 false

位置：

- `zhongshu-core/src/tool/browser_automation.rs:293-317`

现状：

```rust
let result = browser.evaluate(...).await?;

Ok(json!({
    "found": result == "true" || result == "True",
}))
```

问题：

- JS 里 `resolve(true)` / `resolve(false)` 返回的是 JSON boolean。
- Rust 侧用字符串 `"true"` / `"True"` 比较。
- 如果 `result` 是 `Value::Bool(true)`，比较字符串会失败。

影响：

- 元素已经出现时也可能返回 `found: false`。
- 前端调试、表单填写、SPA 等待加载都会被误判失败。

建议：

```rust
let found = result.as_bool().unwrap_or(false);
```

并把 timeout 限制到合理范围，避免 LLM 传入过大等待时间。

验收：

- 单测覆盖 `Value::Bool(true)`。
- 实测一个延迟插入 DOM 的页面。

---

### P1：eval 风险等级过低

位置：

- `zhongshu-core/src/tool/browser_automation.rs:732-738`

现状：

```rust
"open" | "snapshot" | "eval" | "console" | "wait" | "scroll" | "screenshot" => "read",
```

问题：

- `eval` 可以执行任意 JS。
- 它不只是读取，也可以：
  - 修改 DOM。
  - 点击按钮。
  - 读取 localStorage/sessionStorage。
  - 提交表单。
  - 发起网络请求。
  - 删除页面状态。

影响：

- 上层 planner / Runbook / 审批逻辑会认为 `eval` 是只读。
- 在企业私有代码、网页自动化和外部副作用场景中风险被低估。

建议：

- 将 `eval` 标记为 `dangerous` 或至少 `interact`。
- 更好：拆成两个动作：
  - `eval_readonly`：只允许表达式，禁止赋值/调用危险 API，仍需 best-effort。
  - `eval`：危险动作，需要确认或强审计。

验收：

- Runbook 中 `eval` 不再归类为 read。
- 需要写入页面状态的 eval 经过危险工具审批。

---

### P2：装备启停只改内存，不刷新 prompt，也不持久化

位置：

- `zhongshu-orb/src/handler.rs:154-163`

现状：

```rust
if let Some(eq) = equipment.get_mut(&eq_id) {
    eq.status = if matches!(eq.status, Active) {
        Disabled
    } else {
        Active
    };
}
```

问题：

- 只改内存状态。
- 没有调用 `controller.refresh_skill_prompts()`。
- 没有保存 disabled 状态到磁盘。
- UI 也没有立即重新拉取列表或 toast 反馈。

影响：

- 用户在 UI 中禁用装备后，当前 system prompt 可能仍包含该 skill。
- 重启后装备状态恢复为 active。
- 装备管理 UI 看起来能操作，但实际效果不完整。

建议：

- toggle 后调用 `controller.refresh_skill_prompts()`。
- 为装备状态增加持久化，例如：
  - manifest 增加 status。
  - 或单独 `equipment_state.json`。
- toggle 后刷新列表并 toast。

验收：

- 禁用 skill 后，新对话 system prompt 不再包含该 prompt。
- 重启后 disabled 状态保持。

---

### P2：Runbook 只有 schema/store，没有写入闭环

位置：

- `zhongshu-core/src/core/runbook.rs`
- `zhongshu-core/src/core/db.rs`

现状：

- 新增 `runbooks` 和 `runbook_steps` 表。
- 新增 `RunbookStore::save/list`。
- 但项目里没有实际调用 `RunbookStore::save`。

问题：

- 自动化任务不会生成 Runbook。
- 浏览器动作不会记录为 Runbook step。
- 权限确认、verification、失败恢复没有进入 artifact。

影响：

- “Runbook artifact”目前还不是功能闭环，只是数据结构和表结构。

建议：

- 在 Agent loop 或 browser planner 层采集 tool call。
- 每次任务结束生成 Runbook。
- 记录：
  - goal/source。
  - tool action。
  - input 摘要。
  - observation 摘要。
  - verification。
  - risk gate。

验收：

- 完成一次浏览器自动化任务后，`runbooks` 表有记录。
- UI 或 debug panel 能看到 runbook。

---

### P2：browser_session 模块不稳定且未接入主路径

位置：

- `zhongshu-core/src/tool/browser_session.rs`

问题：

- `try_connect()` 名字是 connect，但实现调用的是 `Browser::launch(config)`。
- `Browser::launch` 返回的 `_handler` 被立即丢弃，chromiumoxide 通常需要持续驱动 handler future。
- 现有 `browser_automation` 仍使用自己的 `ManagedBrowser`，这个新模块没有接入主路径。
- `cargo check` 报 `profile_dir` never read。

影响：

- 当前可能只是未完成 scaffolding。
- 如果后续切到该模块，可能出现浏览器连接不稳定、事件不处理、资源生命周期不清晰。

建议：

- 要么暂时删除未接入模块。
- 要么补完整：
  - 真实 connect vs launch。
  - handler task 生命周期。
  - shutdown / Drop。
  - 与 `browser_automation` 合并。

---

## Verification

已执行：

```bash
cargo check -q
```

结果：通过。

Warnings：

```text
warning: unused imports: `ChatCompletionRequest` and `Message`
 --> zhongshu-core/src/agent/llm_registry.rs:1:25

warning: field `profile_dir` is never read
  --> zhongshu-core/src/tool/browser_session.rs:12:5

warning: unused variable: `llm_registry`
   --> zhongshu-orb/src/main.rs:269:9
```

这些 warning 不是单纯清洁问题，其中 `llm_registry` unused 直接对应多 LLM 功能未接入。

## 建议修复顺序

1. 修浏览器动作确定性 bug。
   - `scroll`
   - `wait_for_selector`
   - `eval` 风险等级

2. 接通多 LLM 运行链路。
   - 主 Agent role。
   - worker role。
   - 后台服务 role。
   - provider/model 唯一权威来源。

3. 修装备启停闭环。
   - refresh prompt。
   - 持久化状态。
   - UI 反馈。

4. 明确 Runbook 是 scaffolding 还是已完成。
   - 如果是已完成目标，需要补实际写入链路。
   - 如果是 scaffolding，Roadmap/文档要避免宣称闭环。

5. 清理或接入 `browser_session`。
   - 避免长期保留未接入且生命周期不完整的模块。
