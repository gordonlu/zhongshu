use crate::tool::{Tool, ToolOutput};
use async_trait::async_trait;
use serde_json::json;

#[derive(Debug, Clone, serde::Deserialize)]
struct TestStep {
    name: String,
    tool: String,
    args: serde_json::Value,
    expect_contains: Option<String>,
    optional: Option<bool>,
}

pub struct SelfTestTool;

#[async_trait]
impl Tool for SelfTestTool {
    fn name(&self) -> &str {
        "self_test"
    }

    fn description(&self) -> &str {
        "Run a list of integration tests. Each step calls a real tool and checks the result. Reports pass/fail per step."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "steps": {
                    "type": "array",
                    "description": "测试步骤列表",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string", "description": "步骤名称"},
                            "tool": {"type": "string", "description": "工具名"},
                            "args": {"type": "object", "description": "工具参数"},
                            "expect_contains": {"type": "string", "description": "预期输出包含的文本（可选）"},
                            "optional": {"type": "boolean", "description": "失败是否不影响结果（可选）"}
                        },
                        "required": ["name", "tool", "args"]
                    }
                }
            },
            "required": ["steps"]
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let steps: Vec<TestStep> = match serde_json::from_value(arguments["steps"].clone()) {
            Ok(s) => s,
            Err(e) => return ToolOutput::error(format!("steps 参数格式错误: {e}")),
        };

        if steps.is_empty() {
            return ToolOutput::error("至少需要一个测试步骤");
        }

        let registry = crate::tool::default_registry()
            .register(crate::tool::fs::ReadFileTool)
            .register(crate::tool::fs::WriteFileTool)
            .register(crate::tool::shell::ShellTool)
            .register(crate::tool::search_files::SearchFilesTool)
            .register(crate::tool::webfetch::WebFetchTool)
            .register(crate::tool::system_info::SystemInfoTool)
            .register(crate::tool::memory::MemoryTool::new(
                std::env::var("HOME")
                    .map(|h| std::path::PathBuf::from(h).join(".config/zhongshu/agent.json"))
                    .unwrap_or_default(),
            ))
            .register(crate::tool::search::WebSearchTool);

        let mut results = Vec::new();
        let mut passed = 0u32;
        let mut failed = 0u32;

        for step in &steps {
            let args_str = serde_json::to_string(&step.args).unwrap_or_default();
            let output = registry.execute(&step.tool, &args_str).await;

            let ok = match output.status {
                crate::tool::ToolStatus::Success => {
                    if let Some(ref expect) = step.expect_contains {
                        output
                            .data
                            .as_ref()
                            .map(|d| d.to_string().contains(expect))
                            .unwrap_or(false)
                    } else {
                        true
                    }
                }
                _ => false,
            };

            if ok {
                passed += 1;
            } else if step.optional.unwrap_or(false) {
                passed += 1;
            } else {
                failed += 1;
            }

            results.push(json!({
                "name": step.name,
                "tool": step.tool,
                "status": if ok { "pass" } else { "fail" },
            }));
        }

        let total = steps.len();
        let report = format!(
            "# 自检报告\n\n通过 {passed}/{total}，失败 {failed}\n\n{}",
            results
                .iter()
                .map(|r| {
                    let icon = if r["status"] == "pass" { "✅" } else { "❌" };
                    format!(
                        "- {} {} — {}",
                        icon,
                        r["name"].as_str().unwrap_or("?"),
                        r["tool"].as_str().unwrap_or("?")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        );

        ToolOutput::success(json!({
            "report": report,
            "passed": passed,
            "failed": failed,
            "total": total,
            "details": results,
        }))
    }
}
