# 中书 Zhongshu

中书是一个长期运行在个人电脑上的 Agent Runtime。它不是单纯的聊天窗口，而是把对话、长期记忆、任务、工具调用、权限审批和桌面 UI 放在同一个本地运行时里。

当前项目仍处于早期迭代阶段，核心能力已经可用，但浏览器自动化、多模态感知和多 Agent 调度还在继续完善。

## 功能概览

- 桌面悬浮聊天窗口：基于 `wry` + GTK WebView。
- CLI 入口：用于命令行对话和工具调用验证。
- OpenAI-compatible LLM Provider：默认面向 DeepSeek API。
- 流式 ReAct Agent Loop：支持工具调用、流式输出和停止生成。
- 本地持久化：SQLite 存储 goals、tasks、observations、memories、events 等核心状态。
- 任务系统：Goal -> Task -> Step -> Run 的基础闭环。
- 记忆系统：候选记忆、策略评估、长期记忆和语义检索。
- 权限系统：按工具风险分级，危险操作需要用户确认。
- Web 工具：搜索、网页抓取、浏览器辅助读取。
- Chrome 自动化：通过中书托管的 Chrome profile 执行打开页面、DOM snapshot、JS eval、点击、输入、console 读取。
- UI 能力：深色/浅色主题、2X 窗口放大、停止按钮、任务面板、设置面板。
- 自我进化装备：基于内部使用模式提议并安装可复用 skill 装备。

## 当前边界

- Chrome 自动化目前基于 DevTools Protocol，适合受控 profile 的调试、填表和页面操作；对复杂登录态、强反自动化站点和跨浏览器兼容还不是完整方案。
- Screenshot/多模态链路不是当前主要依赖，后续会随模型多模态能力继续接入。
- 工具输出已经做 observation 边界转义和网页文本清洗，但网页内容仍必须视为不可信外部输入。
- 全量测试在部分沙箱环境下可能受本地端口绑定或全局权限状态影响，相关单测需要按模块运行验证。

## 仓库结构

```text
zhongshu-core
  Agent loop、LLM provider、工具、权限、任务/记忆/事件等核心逻辑

zhongshu-orb
  桌面端入口、WebView UI、配置、全局快捷键、后台服务

zhongshu-cli
  命令行入口

zhongshu-message-core
  消息相关共享类型
```

## 环境要求

- Rust 1.80+
- Linux 桌面环境需要 GTK/WebKitGTK 相关运行库，具体包名随发行版不同。
- Chrome、Chromium、Edge 三者之一，用于 `browser_automation`。
- DeepSeek API key，或兼容 OpenAI Chat Completions API 的服务。

## 快速开始

```bash
cargo check
```

CLI：

```bash
export DEEPSEEK_API_KEY="你的 API Key"
cargo run -p zhongshu-cli
```

桌面端：

```bash
export DEEPSEEK_API_KEY="你的 API Key"
cargo run -p zhongshu-orb
```

也可以在桌面端设置页保存 API key。中书会优先读取环境变量；如果环境变量不存在，则从系统凭据存储读取。API key 不会写入 `config.json`。

## 常用配置

默认配置由 `zhongshu-orb` 管理，关键项包括：

- `llm.api_key_env`：API key 环境变量名，默认 `DEEPSEEK_API_KEY`。
- `llm.model`：默认模型，默认 `deepseek-v4-flash`。
- `llm.api_base`：API base，默认 `https://api.deepseek.com`。
- `llm.model_routing`：Flash/Pro 自动路由配置。
- `deeplossless.proxy_port`：deeplossless 本地代理端口，默认 `8081`。
- `agent.auto_evolve`：自我进化装备开关。

Chrome 自动化环境变量：

- `ZHONGSHU_CHROME_BIN`：指定 Chrome/Chromium/Edge 可执行文件路径。
- `ZHONGSHU_CHROME_PORT`：指定 DevTools 端口，默认 `9223`。
- `ZHONGSHU_CHROME_PROFILE`：指定中书托管 Chrome profile 目录，默认 `~/.config/zhongshu/chrome-profile`。

CLI 额外支持：

- `ZHONGSHU_MODEL`：覆盖 CLI 默认模型。

## 安全模型

中书默认把工具分为不同风险等级。文件写入、shell、桌面自动化、浏览器自动化等敏感能力会经过权限检查和用户确认。

外部网页内容的处理原则：

- `webfetch` / `browser` 会清理零宽字符、控制字符和明显乱码。
- 工具 observation 输出会转义 `<`、`>`、`&`，避免网页内容伪造 observation 边界。
- `browser_automation` 的 DOM、JS eval 和 console 返回值会递归清洗字符串。
- 模型系统提示会明确要求把网页内容视为不可信数据，而不是可执行指令。

这些措施不能让任意网页内容变成可信内容。涉及发帖、提交表单、转账、删除、发邮件等外部副作用时，应在最终动作前由用户确认。

## 开发与验证

格式检查：

```bash
cargo fmt --check
```

编译检查：

```bash
cargo check
```

运行核心相关单测：

```bash
cargo test -p zhongshu-core
```

针对浏览器自动化工具的单测：

```bash
cargo test -p zhongshu-core tool::browser_automation::tests
```

## 许可证

本项目使用 MIT License。详见 [LICENSE](LICENSE)。
