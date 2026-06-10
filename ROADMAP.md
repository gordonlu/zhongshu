# 中书 (Zhongshu) Roadmap

## Vision

中书不是聊天窗口。

中书是一个长期运行在用户电脑上的 Agent Runtime。

它拥有：

* 长期记忆
* 持续任务
* 工具调用能力
* 桌面感知能力
* 主动执行能力

用户不是每次打开中书，而是中书始终存在。

---

# Architecture

```
zhongshu-core       Rust lib — 纯逻辑，无 UI 依赖
├── agent/              LLM provider + ReAct loop + streaming + guardrails
├── event/              EventBus (broadcast) + ResponseStream (bounded mpsc) + MessageId
├── memory/             deeplossless DAG 记忆存储 + 压缩
├── task/               Trigger trait + Scheduler + Worker
├── tool/               shell / fs / search / browser / screenshot / automation
└── integration/        ContextEngine facade

zhongshu-cli         CLI binary — 交互式命令行

zhongshu-orb         Desktop UI
├── app.rs              AgentController (lifecycle) + BackgroundRunner + AgentInbox
├── agent.rs            AgentMemory (长期记忆: identity + goals + todos)
├── config.rs           统一配置 (llm/hotkey/ui/agent)
├── event/              EventBus + ResponseStream (in core, shared)
├── gpu.rs              GpuContext (共享) + WindowSurface (每窗口)
├── hotkey.rs           全局快捷键 Win+; (可配置)
├── indicator.rs        StatusIndicator (Windows orb / Linux tray)
├── overlay.rs          Markdown 渲染 + Tool 时间线
└── main.rs             入口 + winit event loop
```

---

# Core Principles

## Agent First

UI 是可替换的。Agent Runtime 是核心。

CLI、Desktop、未来 VSCode Plugin 都共享同一 Runtime。

## Memory Native

所有行为默认进入记忆系统。用户不需要主动保存。

## Event Driven

```text
EventBus (broadcast, system notifications)   → 允许丢
ResponseStream (mpsc bounded, UI streaming)  → 不丢
```

UI、Task、Memory 解耦。

## Streaming Everywhere

token streaming / tool streaming / progress streaming，统一协议。

---

# Phase 1 — Foundation ✅

## Agent Runtime
* OpenAI Compatible Provider
* Streaming
* ReAct Loop
* Tool Execution
* Budget Control + Failure Recovery

## Memory Runtime
* deeplossless integration
* DAG conversation storage + context retrieval + compression

## Tools
* Shell / Filesystem / Search / Browser / Screenshot / Automation

## Task Runtime
* Reminder / Interval / FileWatch

## CLI
* Interactive Chat + Streaming Output

---

# Phase 2 — Desktop Runtime ✅

## EventBus ✅

统一事件总线，发布/订阅，UI / Task / Memory 解耦。ResponseStream 独立 mpsc channel（bounded，backpressure 安全）。

## Status Indicator ✅

```
Windows: OrbIndicator (winit + softbuffer 透明悬浮球)
Linux:   TrayIndicator (tray-icon / GTK / D-Bus)
```

## Overlay ✅

egui + wgpu 对话框。Markdown 渲染（LayoutJob stack-based）、Code block（monospace）、Tool 时间线（Running/Done 状态 + 耗时）。流式文本缓存（content hash），CJK 字体自动加载。

## Global Hotkey ✅

Win+; 默认，可配置（`~/.config/zhongshu/hotkey.json`）。跨平台 global-hotkey crate。

## 架构硬化 ✅

统一配置系统（原子写入，API key 不入盘）、GPU 资源生命周期（WindowSurface + Drop）、聊天历史上限（max_chat_entries 截断）、streaming 超时检测、EventBus lagged 可观测、drain() reducer pipeline、AgentController UI 解耦。

---

# Phase 3 — Agent OS 🏗️

## Persistent Agent ✅

Agent 拥有长期身份和记忆。

```
AgentProfile  (identity + active_goals + todos)
     ↓
 每次对话注入 system prompt（活跃目标 + 待办列表）
     ↓
 响应后自动解析 - [ ] checkbox → 提取 TodoItem → 持久化
```

## Background Runner ✅

定时自主检查。处于 Idle 时才触发，防止重叠执行。

## Agent Inbox ✅

统一入口：用户消息和后台任务通过同一队列排队，Agent Idle 时逐条分发。

```
用户输入 ─┐
          ├→ AgentInbox (VecDeque) → AgentController.run()
后台任务 ─┘          ↑
            EventBus Idle 事件触发 drain
```

## System Safety & Privacy ✅

### Authority Gate v1.1
统一权限关卡（单例 `GLOBAL_GATE`），覆盖 shell / screenshot / automation / browser 所有工具。

```
4 级风险: Safe → Moderate → Dangerous → Blocked
缓存:     批准后 30 分钟免审（keyed by tool + program）
审计:     audit.log 追加写入（ALLOW / BLOCK / GRANT / DENY）
```

### System Integrity
- 磁盘/分区工具（`format`、`mkfs`、`dd of=/dev`）→ **Blocked**
- 系统路径写入（`/etc/`、`/boot/`、`/sys/`、`/usr/`、`/lib*`、`/bin/`、`/sbin/`）→ **Blocked**
- 提权绕过防护（`sudo`/`pkexec`/`doas` + 破坏性操作）→ **Blocked**
- 命令链绕过防护（`&&`、`;` 多命令拼接）→ **Dangerous**
- 递归根删除（`rm -rf /`、`rm -rf ~`）→ **Blocked**
- 误报控制：`/`lib` 匹配不误伤 `/libreoffice`

### Privacy
- 敏感路径检测：`~/.ssh/`、`~/.gnupg/`、`~/.aws/`、`~/.kube/`、`~/.config/zhongshu/` → **Dangerous**
- 任何涉及敏感路径的命令都需用户批准（包括 `cat`、`ls` 等只读命令）
- `/.ssh` 前缀匹配避免 `find -name .ssh` 误报
- 嵌入路径检测（`dd of=~/.ssh/authorized_keys` → 危险）
- 系统提示词注入防御：忽略网页/文件中的恶意指令，不读取私密文件，不外发数据

### UI/UX
- 输入响应修复：`Poll` + `request_redraw()` 替代 `WaitUntil`，消除 Wayland 输入延迟
- Overlay 视觉重设计：温暖极简主题（terracotta 主色 #d04a1a、卡式对话气泡）
- 权限审批机制：LLM 检测用户确认词（yes/可以/确认）→ 调用 `approve_pending()`
- 标签泄露修复：`<final_answer` 无 `>` 断块也能被 strip

## 待完成

* 长期目标生命周期管理：GoalStatus::Archived、结构化记忆 schema
* 任务调度器完整接入：Reminder/FileWatch → inbox 路由
* 桌面通知系统：notify-rust 已引入，未完全接入
* AuthorityGate UI 审批对话框：当前是 LLM 中介模式，未来 overlay 直接展示审批界面

---

# Phase 4 — Multi-Agent

* Planner: 拆任务
* Researcher: 搜索
* Executor: 工具执行
* Reviewer: 检查结果
* 共享 deeplossless 记忆系统

---

# Phase 5 — Knowledge Operating System

* Personal Knowledge Graph（自动构建实体/关系图谱）
* Memory Compaction v2（对话压缩 → 知识提取）
* Context Streaming（边推理边检索边补充）
