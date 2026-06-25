# 中书 (Zhongshu) Roadmap

## Vision

中书不是聊天窗口。中书是一个长期运行在用户电脑上的 Agent Runtime。

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
├── core/               核心领域模型 + SQLite 持久化
│   ├── goal/               目标管理 (repository + tool)
│   ├── task/               任务管理 (repository + tool + planner + executor)
│   ├── observation/        观察存储 (auto-fed from EventBus)
│   ├── suggestion/         建议引擎 (pattern + LLM-based)
│   ├── memory/             记忆管道 (candidate → policy → memory)
│   ├── artifact/           资料存储
│   ├── scheduler/          Goal→Task 定时触发器
│   ├── event/              Event 持久化日志
│   ├── db.rs               SQLite WAL-mode 数据库
│   ├── models.rs           13 个核心模型
│   └── tests.rs            19 个集成测试
├── agent/              LLM provider + ReAct loop + streaming + guardrails
├── event/              EventBus (broadcast) + ResponseStream (mpsc)
├── integration/        deeplossless DAG 记忆存储
├── tool/               shell / fs / search / browser / screenshot / search_files / memory
├── authority/          4 级权限关卡 (Safe/Moderate/Dangerous/Blocked)
└── equipment/          军器监 (search-files skill → 已迁移为 tool)

zhongshu-orb         Desktop UI (wry + GTK WebView)
├── app.rs              AgentController (lifecycle) + spawn_task + AgentInbox
├── agent.rs            AgentMemory (goals + todos + long_term_memory)
├── config.rs           统一配置 (llm/hotkey/ui/agent/equipment)
├── handler.rs          winit ApplicationHandler + overlay management
├── main.rs             入口 + winit event loop + EventBus wiring
├── overlay.rs          WebView IPC + task UI + auth dialog
├── indicator.rs        Linux tray / Windows orb
├── render.rs           Windows orb 纯数学渲染（softbuffer）
├── services.rs         后台服务 (scheduler/memory/suggestion/evolve)
├── hotkey.rs           全局快捷键
└── assets/chat.html    HTML/CSS/JS 前端
```

---

# Core Principles

## Agent First

UI 是可替换的。Agent Runtime 是核心。

## Memory Native

所有行为默认进入记忆系统。用户不需要主动保存。

## Event Driven

```
EventBus (broadcast) → 观察、任务、记忆全部事件驱动
```

UI、Task、Memory 解耦。

## Streaming Everywhere

token streaming / tool streaming，统一协议。

---

# Phase 1 — Foundation ✅

## Agent Runtime
* OpenAI Compatible Provider
* Streaming + ReAct Loop
* Tool Execution + Budget Control
* deeplossless DAG memory

## Core Tools
* Shell / Filesystem / WebSearch / Browser / ScreenShot / Automation

---

# Phase 2 — Desktop Runtime ✅

## wry + GTK WebView
替换 egui/wgpu，使用系统 WebView 渲染 chat HTML。

## EventBus + Overlay IPC
统一事件总线，WebView 双向 IPC。

## Auth Dialog
授权审批 UI（inline bar，非 modal 遮挡）。

## 架构硬化
统一配置、原子写入、API key 环境变量、streaming 超时检测。

---

# Phase 3 — Agent Operating System ✅

## Persistent Agent
```
AgentProfile → 每次对话注入 system prompt
响应后自动解析 todo/goal checkbox → 持久化
```

## Core Database (core.db) ✅
10 张 SQLite 表：observations / suggestions / goals / tasks / task_steps / task_runs / artifacts / task_artifacts / memory_candidates / memories / events

## Goal/Task 生命周期 ✅
```
LLM goal tool → GoalRepository → Scheduler → Task → Executor → Artifact → MemoryCandidate
全部经 EventBus 驱动
```

## 自动记忆沉淀 ✅
```
Observation → EventBus → MemoryCandidate (confidence≥0.7) → MemoryPolicy.evaluate() → Memory
后台任务每 600s 评估一次
```

## 建议引擎 ✅
```
ObservationStore → LLM 分析 → SuggestionEngine → SuggestionTool
模式分析 + LLM 分析双通道，每 1800s 运行
```

## 权限关卡 ✅
4 级风险 + TTL 缓存 + 审计日志

## UI 任务面板 ✅
Header 📋 按钮 → 弹出任务列表，支持完成/取消

---

# Phase 4 — Intelligence ✅

* ✅ **LLM Planner** — TaskPlanner 从硬编码模板升级为 LLM 生成执行计划
* ✅ **Task Step 执行** — 按步骤逐步执行，更新 step status
* ✅ **Suggestion→Goal 自动转化** — 高置信度建议自动创建目标（compensation service）
* ✅ **Memory 向量检索** — embedding 列 + 语义搜索（cosine similarity，fallback LIKE）
* ✅ **Event log 持久化** — EventBus 事件入库，支持 replay/debug，10MB 自动截断
* ✅ **DeepSeek V4 优化** — model routing（Flash/Pro）、reasoning_effort、thinking mode
* ✅ **上下文压缩** — 500k threshold、80% 触发、deeplossless DAG compress
* ✅ **Auto-evolve（装备自动进化）** — observer → LLM proposal → 安装 → 热刷新 system prompt
* ✅ **Human delay** — web 工具统一 1-3s 随机延迟
* ✅ **Cookie 持久化 + 并发控制** — 跨请求共享 cookie jar，max 3 并发
* ✅ **安全页面检测** — 验证码/反爬页面识别，返回警告而非乱码
* ✅ **防注入** — sanitize_web_content 过滤零宽字符 + 控制字符 + 乱码检测
* ✅ **编码修复** — decode_html 自动检测 charset，extract_text 逐字符解码

---

# Phase 5 — Platform

* ✅ **CI 流水线** — fmt/clippy/check/cross-platform test
* ✅ **Wayland 兼容** — desktop 工具 ydotool/wtype fallback
* ✅ **Windows orb** — 纯数学渲染的 Siri 风格球体，softbuffer + wry
* ✅ **Tray 优化** — 自适应呼吸频率（idle 2Hz / active 20Hz）
* ✅ **Chrome CDP 集成** — 通过 DevTools Protocol 控制浏览器

---

# Phase 6 — 浏览器完全自动化 ✅

## 6A: Browser Session + 可靠页面操作 ✅
* ✅ 16 个 CDP 操作原语（open/snapshot/eval/click/type/console/wait/scroll/back/forward/new_tab/press/wait_for_selector/select_option/screenshot）
* ✅ CDP 截图、network 捕获、page errors 捕获
* ✅ KillOnDrop 防 Chrome 进程泄漏
* ✅ action_risk 风险分类
* ✅ action 后工具输出自动 attach risk metadata

## 6B: 副作用安全闭环 ✅
* ✅ `action_risk()` 函数分类 read/interact/navigate/dangerous
* ✅ eval 标记为 dangerous（可执行任意 JS）
* ✅ 各操作通过 authority gate 统一授权审批
* ⬜ 外部写入前用户确认 UI（Phase 6A 工具层已备，UI 层待补）

## 6C: 视觉观测 + 网页调试 ✅
* ✅ CDP 截图（base64 PNG）
* ✅ network_start / network_events（fetch/XHR 拦截，防重复包装）
* ✅ page_errors（addEventListener 防覆盖、守夜人初始化）
* ⬜ HAR 导出
* ⬜ 前端调试工作流（FrontendDebugSession）

## 6D: Web Skill 记忆 ✅
* ✅ Runbook 数据结构 + SQLite 持久化（runbooks + runbook_steps 表）
* ✅ Runner 自动写入（task executor 完成后保存 Runbook）
* ⬜ skill 提炼（Runbook→Equipment）

---

# Phase 7 — 多 LLM 配置 + Worker 专业化 ✅

## LlmRegistry ✅
* ✅ LlmProfileConfig 多 profile 配置
* ✅ Role mapping（primary / worker.* / background.*）
* ✅ Fallback 链（role → worker.default → default）
* ✅ 配置迁移（旧单 profile 格式兼容）
* ✅ LlmRegistry 在 main.rs 实例化

## AgentProfile 改造 ✅
* ✅ profile.llm 字段（llm_profile / llm_model / llm_reasoning_effort）
* ✅ Worker 根据 profile 切换 provider（change_model trait 方法）
* ✅ Provider model 唯一权威来源（build_body 用自身 model）
* ⬜ 后台服务按 role 取模型（memory / suggestion / auto-evolve 仍用同一 provider）

## UI 设置分层 ⬜
* 基础页/高级页尚未实现

---

# Phase 8 — 模式切换 ✅

## 助手 / 编码模式切换 ✅
* ✅ mode 配置字段 + UI 切换按钮
* ✅ IPC 全链路（button → overlay → handler → config → prompt refresh）
* ✅ System prompt 按 mode 过滤 skill（`[coding]` / `[assistant]` 标签）
* ✅ 编码模式窗口 860×900 vs 助手 520×800
* ⬜ Tool registry 按 mode select
* ⬜ 编码模式专属 UI（git diff / test result / workspace）

---

# Phase 9 — 本地 Agent Debugger （骨架）

* ✅ Debug 面板 UI（chat.html renderDebug）
* ✅ IPC push_debug overlay 方法
* ⬜ 时间线 / tool call 回放 / cost / latency

---

# Phase 10 — 自我进化装备市场（骨架）

* ✅ 装备管理 UI（列表 + 启用/禁用按钮）
* ✅ list_equipment / toggle_equipment IPC 全链路
* ✅ toggle 后自动 refresh_skill_prompts
* ⬜ 装备启停持久化（当前不保存到磁盘）
* ⬜ 安装预览 / 版本回滚 / 测试
