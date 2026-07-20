use crate::authority::{self, CheckResult};
use crate::tool::{Tool, ToolOutput, ToolStatus};
use async_trait::async_trait;
use serde_json::json;
use tokio::process::Command;
use tracing::info;

pub struct ShellTool;

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return stdout/stderr."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "The shell command to execute"},
                "cwd": {"type": "string", "description": "Working directory (optional)"}
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let command = match arguments["command"].as_str() {
            Some(c) => c,
            None => return ToolOutput::error("'command' must be a string"),
        };

        // Authority gate check.
        let result = authority::check("shell", command);
        match result {
            CheckResult::Deny { reason } => {
                return ToolOutput::error(format!("[BLOCKED] {reason}"));
            }
            CheckResult::RequireAuth { request } => {
                let rid = authority::set_pending(&request.tool, &request.program, &request.command, "");
                return ToolOutput::auth_required(&request.program, &request.command).with_request_id(rid);
            }
            CheckResult::Allow => {}
        }

        let cwd = arguments["cwd"].as_str();
        let (shell, flag) = if cfg!(target_os = "windows") {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };

        let mut cmd = Command::new(shell);
        cmd.arg(flag).arg(command);
        cmd.kill_on_drop(true);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        info!("shell: {command}");

        match cmd.output().await {
            Ok(output) => {
                let exit_code = output.status.code().unwrap_or(-1);
                let data = json!({
                    "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                    "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                    "exit_code": exit_code,
                });
                if output.status.success() {
                    ToolOutput::success(data)
                } else {
                    ToolOutput {
                        status: ToolStatus::Error,
                        data: Some(data),
                        error: Some(format!("命令失败，退出码: {exit_code}")),
                        auth_program: None,
                        auth_command: None,
                        external_source: false,
                        request_id: None,
                    }
                }
            }
            Err(e) => ToolOutput::error(format!("命令执行失败: {e}")),
        }
    }
}

pub fn approve(tool: &str, program: &str) {
    authority::approve(tool, program);
}

pub fn deny(_tool: &str, _program: &str) {}
