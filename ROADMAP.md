# 中书 (Zhongshu) Roadmap

一个桌面 AI 助手——右下角状态球，随时唤醒，能聊天、执行命令、搜网页、有长期记忆、能操作电脑。

## 架构

```
zhongshu-core/     Rust lib — 大脑
  agent/              LLM provider + ReAct loop + streaming + guardrails
  tool/               shell / fs / search / browser / screenshot / automation
  desktop/            clipboard / notification / os
  integration/        deeplossless ContextEngine (DAG memory + compression)
  task/               Trigger trait + Scheduler + Worker (reminder/watch/interval)

zhongshu-cli/      CLI binary — 交互式命令行, streaming 输出

zhongshu-orb/      Desktop UI — winit 状态球 + egui 对话框
```

## 进展

### Phase 1 — 核心链路 ✅

- [x] `OpenAiProvider` — DeepSeek API, streaming SSE
- [x] `AgentLoop` — ReAct: LLM → tool → observation → next
- [x] 三种 guardrails: execution budget / tool failure handling / `<final_answer>` stop
- [x] 6 个工具: shell, read_file, write_file, list_dir, web_search(DuckDuckGo), browser, screenshot, desktop automation
- [x] `ToolOutput` schema 化 + observation 标记
- [x] deeplossless 集成: DAG 记忆存储, 上下文装配, 压缩触发
- [x] `TaskScheduler` + `Trigger` trait + `ReminderTrigger` / `IntervalTrigger` / `FileWatchTrigger`
- [x] CLI 可交互使用

### Phase 2 — Desktop UI 🏗️

- [x] winit + softbuffer 右下角状态球 (64x64, AlwaysOnTop)
- [x] 5 种状态渲染: idle / listening / thinking / executing / done
- [x] 点击球 → 弹出 egui 对话框窗口
- [ ] egui 对话框编译通过 (wip — winit + egui + wgpu 版本适配中)
- [ ] 流式响应实时显示在对话框
- [ ] 全局快捷键唤醒 (Ctrl+Space)

### Phase 3 — 升级 ❄️

- [ ] 语义压缩 (topic shift detection + DAG subtree 压缩)
- [ ] MCP 外部工具接入
- [ ] 流式 early tool trigger
- [ ] 对话框中文 IME 支持
- [ ] Tauri 壳 (tray icon + 系统通知)

## 技术栈

| 层 | 技术 |
|----|------|
| 语言 | Rust (edition 2021) |
| 异步 | tokio |
| LLM | DeepSeek (OpenAI-compatible) |
| 记忆 | deeplossless (SQLite + DAG) |
| CLI | stdin/stdout + streaming |
| 桌面 UI | winit + softbuffer (orb) + egui + wgpu (overlay) |
| 字体 | fontdue (pending) |

## 运行

```bash
export DEEPSEEK_API_KEY=sk-your-key
# CLI 模式
cargo run -p zhongshu-cli

# Desktop 模式 (WIP)
cargo run -p zhongshu-orb
```
