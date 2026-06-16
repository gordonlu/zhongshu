use crate::tool::{Tool, ToolOutput};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TestManifest {
    steps: Vec<TestStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TestStep {
    name: String,
    tool: String,
    args: HashMap<String, serde_json::Value>,
    /// Optional substring expected in stdout/data content.
    expect_contains: Option<String>,
    /// If true, step failure is non-fatal.
    optional: Option<bool>,
}

pub struct SelfTestTool;

#[async_trait]
impl Tool for SelfTestTool {
    fn name(&self) -> &str {
        "self_test"
    }

    fn description(&self) -> &str {
        "Run a self-test suite from a manifest JSON file. Each step calls a real tool and checks the result. Reports pass/fail per step."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "manifest": {
                    "type": "string",
                    "description": "Path to the test manifest JSON file (default /tmp/zhongshu_self_test.json)"
                }
            },
            "required": []
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let path = arguments
            .get("manifest")
            .and_then(|v| v.as_str())
            .unwrap_or("/tmp/zhongshu_self_test.json");

        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => {
                // Generate default manifest if file doesn't exist.
                let default = serde_json::json!({
                    "steps": [
                        {"name":"读取文件","tool":"read_file","args":{"path":"/etc/hostname"}},
                        {"name":"写入文件","tool":"write_file","args":{"path":"/tmp/zhongshu_self_test.txt","content":"ok"},"expect_contains":"ok"},
                        {"name":"Shell命令","tool":"shell","args":{"command":"echo hello"},"expect_contains":"hello"},
                        {"name":"搜索文件","tool":"search_files","args":{"pattern":"hostname","path":"/etc"}},
                        {"name":"系统信息","tool":"system_info","args":{}},
                    ]
                });
                return ToolOutput::success(json!({
                    "warning": format!("清单文件 {path} 不存在，已生成默认测试清单。请编辑该文件后重新调用。"),
                    "default_manifest": default,
                }));
            }
        };

        let manifest: TestManifest = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => return ToolOutput::error(format!("清单格式错误: {e}")),
        };

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

        for step in &manifest.steps {
            let optional = step.optional.unwrap_or(false);
            let output = registry
                .execute(
                    &step.tool,
                    &serde_json::to_string(&step.args).unwrap_or_default(),
                )
                .await;

            let ok = match output.status {
                crate::tool::ToolStatus::Success => {
                    if let Some(ref expect) = step.expect_contains {
                        output
                            .data
                            .as_ref()
                            .and_then(|d| d.as_str())
                            .map(|s| s.contains(expect))
                            .unwrap_or(false)
                            || output
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
            } else if optional {
                passed += 1; // count optional passes as pass
            } else {
                failed += 1;
            }

            results.push(json!({
                "name": step.name,
                "tool": step.tool,
                "status": if ok { "pass" } else { "fail" },
                "optional": optional,
            }));
        }

        let total = manifest.steps.len();
        let report = format!(
            "# 自检报告\n\n通过 {passed}/{total}，失败 {failed}\n\n{}",
            results
                .iter()
                .map(|r| format!(
                    "- {} {} — {}",
                    if r["status"] == "pass" { "✅" } else { "❌" },
                    r["name"].as_str().unwrap_or("?"),
                    r["tool"].as_str().unwrap_or("?"),
                ))
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
