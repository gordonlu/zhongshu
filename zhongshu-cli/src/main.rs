use std::io::{self, Write};

use anyhow::Context;
use tracing::{info, warn};

use zhongshu_core::agent::llm::{Message, OpenAiProvider};
use zhongshu_core::agent::loop_::{AgentBudget, AgentLoop, StopReason};
use zhongshu_core::tool::default_registry;
use zhongshu_core::integration::{ContextConfig, ContextEngine};

const SYSTEM_PROMPT: &str = "\
你是「中书」(Zhongshu)，一个桌面 AI 助手。你的职责是记录和传达——记住用户的信息，执行用户的操作指令。

## 核心能力
- 执行 shell 命令 (shell)
- 读写文件 (read_file / write_file / list_dir)
- 搜索网页 (web_search)
- 浏览器操作 (browser)
- 桌面自动化 (desktop — 键盘鼠标操作)
- 截图 (screenshot)

## 工具输出格式
所有工具输出使用结构化 observation 标记:
<observation tool=\"name\" status=\"success|error\">{data}</observation>
成功时读取 data 字段，失败时读取 error 字段并决定下一步。

## 停止标记
完成任务时在回复末尾加 <final_answer>。

## 行为准则
- 回复简洁，直接给出结果
- 记住用户的偏好和重要信息
- 用中文回复";

fn status_line(tool: &str, success: bool) {
    let icon = if success { "✓" } else { "✗" };
    eprintln!("\x1b[90m  {icon} {tool}\x1b[0m");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zhongshu_core::agent::loop_=info,warn".into()),
        )
        .with_target(false)
        .without_time()
        .init();

    let api_key = std::env::var("DEEPSEEK_API_KEY")
        .context("请设置环境变量 DEEPSEEK_API_KEY")?;

    let model = std::env::var("ZHONGSHU_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".into());

    println!("中书 v{} · {model}", env!("CARGO_PKG_VERSION"));
    println!("/help 帮助  /exit 退出\n");

    let provider = OpenAiProvider::new(&api_key, &model);
    let tools = default_registry()
        .register(zhongshu_core::tool::search::WebSearchTool)
        .register(zhongshu_core::tool::browser::BrowserTool)
        .register(zhongshu_core::tool::screenshot::ScreenshotTool)
        .register(zhongshu_core::tool::automation::AutomationTool);

    let ctx_config = ContextConfig {
        api_key: api_key.clone(),
        ..ContextConfig::default()
    };

    let context_engine = ContextEngine::new(ctx_config)
        .await
        .context("无法初始化上下文引擎")?;

    let conv_id = context_engine.find_or_create_conv(SYSTEM_PROMPT, &model)?;
    info!(conv_id, "会话已创建");

    let budget = AgentBudget { max_steps: 30, max_tool_calls: 20, token_limit: 50_000 };

    loop {
        print!("\n中书 > ");
        io::stdout().flush()?;

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() { break; }
        let input = input.trim().to_string();
        if input.is_empty() { continue; }

        match input.as_str() {
            "/exit" | "/quit" => { println!("再见。"); break; }
            "/help" => {
                println!("  /exit, /quit  退出");
                println!("  /context      上下文状态");
                println!("  /compact      手动压缩");
                println!("  /budget       执行预算");
                continue;
            }
            "/context" => {
                match (context_engine.conv_token_count(conv_id), context_engine.conv_leaf_count(conv_id)) {
                    (Ok(tok), Ok(leaves)) => {
                        println!("DAG: {tok} tokens, {leaves} 个叶子");
                        let d = context_engine.check_compression(conv_id);
                        if d.should_compress { println!("建议压缩: {:?}", d.reason); }
                    }
                    (Err(e), _) | (_, Err(e)) => println!("查询失败: {e}"),
                }
                continue;
            }
            "/compact" => {
                match context_engine.trigger_compaction(conv_id).await {
                    Ok(_) => println!("已发送"),
                    Err(e) => println!("失败: {e}"),
                }
                continue;
            }
            "/budget" => {
                println!("max_steps={} max_tool_calls={} token_limit={}",
                    budget.max_steps, budget.max_tool_calls, budget.token_limit);
                continue;
            }
            _ => {}
        }

        let memory_ctx = context_engine.build_context(conv_id, 5000, &input).unwrap_or_default();

        let mut messages = Vec::with_capacity(3);
        messages.push(Message::system(SYSTEM_PROMPT));
        if !memory_ctx.is_empty() {
            messages.push(Message::user(format!(
                "<context>\n以下是本次对话之前的历史摘要和近期记录:\n{memory_ctx}\n</context>"
            )));
        }
        messages.push(Message::user(input.clone()));

        let agent = AgentLoop::new(provider.clone(), tools.clone(), model.clone())
            .with_budget(budget.clone())
            .with_messages(messages);

        let input_clone = input.clone();
        let engine = &context_engine;

        let result = agent.run_streaming(
            "",
            move |text| { print!("{text}"); io::stdout().flush().ok(); },
            move |tool| { status_line(tool, true); },
            {
                let input = input_clone.clone();
                move |_tool, success| { if !success { status_line(_tool, false); } let _ = &input; }
            },
        ).await;

        match result {
            Ok(r) => {
                println!();
                if r.stop_reason != StopReason::Finished && r.stop_reason != StopReason::FinalAnswer {
                    println!("[停止: {:?}]", r.stop_reason);
                }
                println!("── {} 次工具 · {} tokens ──", r.tool_calls_made, r.estimated_tokens);

                if let Err(e) = engine.append_turn(
                    conv_id,
                    &format!("[user]: {input}"),
                    &format!("[assistant]: {}", r.messages.last().map(|m| m.content.as_str()).unwrap_or("")),
                ) {
                    warn!(error = %e, "记录对话失败");
                }

                let decision = engine.check_compression(conv_id);
                if decision.should_compress {
                    info!(reason = ?decision.reason, "自动压缩");
                    let _ = engine.trigger_compaction(conv_id).await;
                }
            }
            Err(e) => eprintln!("错误: {e}"),
        }
    }

    Ok(())
}
