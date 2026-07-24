<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="media/banner.webp">
    <img src="media/banner.webp" alt="中书 Zhongshu" width="100%">
  </picture>
</p>

# 中书 Zhongshu

[![CI](https://github.com/gordonlu/zhongshu/actions/workflows/ci.yml/badge.svg)](https://github.com/gordonlu/zhongshu/actions/workflows/ci.yml)
![Rust](https://img.shields.io/badge/rust-1.80+-de5842?logo=rust)
![React](https://img.shields.io/badge/UI-React-61dafb?logo=react)
![License](https://img.shields.io/badge/license-MIT-green)
![Platform](https://img.shields.io/badge/Linux-blue?logo=linux)
![Platform](https://img.shields.io/badge/Windows-blue?logo=windows)
![Platform](https://img.shields.io/badge/macOS-blue?logo=apple)

中书是一个长期运行在桌面端的 Agent Runtime。它不是聊天窗口，而是把对话、编码 agent、长期记忆、任务跟踪、工具调用、权限审批和桌面 UI 放在同一个本地运行时里。

## 功能

- **桌面悬浮窗口** — GTK/Linux、WebView2/Windows、WKWebView/macOS 原生的无边框沉浸式窗口，可拖拽、缩放、深色/浅色主题
- **多 LLM 路由** — 内置 DeepSeek Flash（快速响应）和 Pro（深度推理）自动路由，支持 OpenAI-compatible API
- **Coding Agent** — 面向代码修改的专用 Agent 模式，带架构分析、工作区文件所有权、worker 冲突检测、分步计划、验证门禁
- **上下文管理** — 500K-1M token 长上下文，证据评分与压缩，保留因果链
- **记忆系统** — 候选记忆 → 策略评估 → 长期记忆 → 语义检索
- **任务跟踪** — Goal → Task → Step 闭环，内置 runbook 记录与回放
- **自我进化装备** — Agent 观察你的使用模式，自动提议并安装可复用的 skill 装备
- **Chrome 自动化** — 托管 Chrome profile，16+ DOM/Js eval/console/click/input 原语，kill-on-drop 生命周期
- **安全模型** — 工具按风险分级，危险操作请求用户确认，网页内容不可信处理
- **[Deeplossless](https://github.com/gordonlu/deeplossless) 集成** — 流式上下文压缩，本地代理服务
- **工具集** — Web 搜索、网页抓取、浏览器辅助、Shell、文件编辑、代码搜索

## 快速开始

```bash
# 设置 API key
export DEEPSEEK_API_KEY="sk-xxx"

# 启动桌面端
cargo run -p zhongshu-orb
```

首次启动会在系统托盘显示 orb，点击 orb 或按全局快捷键打开悬浮窗口。

API key 也可以通过窗口内的设置面板保存到系统凭据库。中书优先读环境变量，其次系统凭据，不写入配置文件。

## 环境要求

- Rust 1.80+
- **Linux**：GTK + WebKitGTK (`libgtk-3-dev` `libwebkit2gtk-4.1-dev`)
- **Windows**：WebView2（系统自带 Windows 10+）
- **macOS**：WebKit（系统自带）
- Chrome / Chromium / Edge 三者之一（用于浏览器自动化）
- DeepSeek API key，或兼容 OpenAI Chat Completions API 的服务

## 配置

默认配置存储于 `~/.config/zhongshu/config.json`，关键项：

| 环境变量 | 说明 | 默认值 |
|---|---|---|
| `DEEPSEEK_API_KEY` | API key | — |
| `ZHONGSHU_MODEL` | CLI 模型覆盖 | — |
| `ZHONGSHU_CHROME_BIN` | Chrome 可执行文件路径 | 自动查找 |
| `ZHONGSHU_CHROME_PORT` | DevTools 端口 | `9223` |
| `ZHONGSHU_CHROME_PROFILE` | 托管 Chrome profile | `~/.config/zhongshu/chrome-profile` |

可选后台服务（配置界面开启）：

- **背景工作流** — Agent 在空闲时按计划执行任务
- **自动进化** — 基于使用模式自动提议新 equipment

## 安全

中书按风险等级管理工具调用。文件写入、shell、浏览器自动化等敏感操作需要用户明确确认。网页内容处理时自动清理控制字符、零宽字符、伪造 observation 边界等攻击向量。

## 许可证

MIT
