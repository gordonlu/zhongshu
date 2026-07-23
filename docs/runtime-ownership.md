# Runtime Ownership

## Entry Points

- `zhongshu_core::agent::execute_agent_loop()` / `execute_agent_loop_with_messages()` — 唯一合法的外部入口
- `run_agent()`、`run_agent_with_context()`、`run_agent_with_verification_policy()` — `pub(crate)`
- **禁止** zhongshu-orb 或 zhongshu-cli 直接调用底层 `run_agent*` 函数

## 所有者

| 概念 | 所有者 | 职责 |
|---|---|---|
| Run | `zhongshu-core/src/runtime/record.rs` | 规范状态机 |
| Attempt | `run_attempt()` in `zhongshu-orb/src/app.rs` | 一次连续的 LLM 执行 |
| Action | `zhongshu-core/src/action/dispatcher.rs` | 工具调用生命周期 |
| Cancel | `RunController::request_cancel()` | 统一取消入口 |
| Journal | `ActionJournal` in `zhongshu-core/src/action/journal.rs` | Append-only 记录 |
| Checkpoint | `CheckpointStore` | 崩溃恢复 |

## 验证

```bash
cargo xtask architecture-check
```
