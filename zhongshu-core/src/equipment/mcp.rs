use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::agent::llm::{ToolDef, ToolFunctionDef};
use crate::equipment::manifest::McpServerConfig;
use crate::tool::{Tool, ToolEffect, ToolOutput, ToolSpec, WorkspaceScope};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolDefinition {
    pub server_id: String,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

impl McpToolDefinition {
    pub fn spec(&self) -> ToolSpec {
        ToolSpec::new(&self.name)
            .with_effect(ToolEffect::Unknown)
            .workspace_scope(WorkspaceScope::External)
            .requires_approval(true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPreflightReport {
    pub server_id: String,
    pub tools: Vec<McpToolDefinition>,
    pub error: Option<String>,
}

impl McpPreflightReport {
    pub fn success(server_id: impl Into<String>, tools: Vec<McpToolDefinition>) -> Self {
        Self {
            server_id: server_id.into(),
            tools,
            error: None,
        }
    }

    pub fn failure(server_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            server_id: server_id.into(),
            tools: Vec::new(),
            error: Some(error.into()),
        }
    }
}

#[derive(Clone)]
pub struct McpStdioTool {
    server: McpServerConfig,
    definition: McpToolDefinition,
    working_dir: PathBuf,
}

impl McpStdioTool {
    pub fn new(
        server: McpServerConfig,
        definition: McpToolDefinition,
        working_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            server,
            definition,
            working_dir: working_dir.into(),
        }
    }
}

#[async_trait]
impl Tool for McpStdioTool {
    fn name(&self) -> &str {
        &self.definition.name
    }

    fn description(&self) -> &str {
        &self.definition.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.definition.input_schema.clone()
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let timeout = Duration::from_millis(self.server.timeout_ms.max(1));
        let result = tokio::time::timeout(
            timeout,
            call_tool_once(
                &self.server,
                &self.working_dir,
                &self.definition.name,
                arguments,
            ),
        )
        .await;

        match result {
            Ok(Ok(value)) => ToolOutput::success(value),
            Ok(Err(error)) => ToolOutput::error(error.to_string()),
            Err(_) => ToolOutput::error(format!(
                "MCP tool '{}' timed out after {:?}",
                self.definition.name, timeout
            )),
        }
    }

    fn spec(&self) -> ToolSpec {
        self.definition.spec()
    }

    fn to_tool_def(&self) -> ToolDef {
        ToolDef {
            def_type: "function".into(),
            function: ToolFunctionDef {
                name: self.name().into(),
                description: self.description().into(),
                parameters: self.parameters(),
            },
        }
    }
}

pub async fn preflight_stdio_server(
    server: &McpServerConfig,
    equipment_dir: &Path,
) -> McpPreflightReport {
    if let Err(error) = validate_server(server) {
        return McpPreflightReport::failure(&server.id, error);
    }

    let timeout = Duration::from_millis(server.timeout_ms.max(1));
    match tokio::time::timeout(timeout, list_tools_once(server, equipment_dir)).await {
        Ok(Ok(tools)) => McpPreflightReport::success(&server.id, tools),
        Ok(Err(error)) => McpPreflightReport::failure(&server.id, error.to_string()),
        Err(_) => McpPreflightReport::failure(
            &server.id,
            format!("MCP server preflight timed out after {:?}", timeout),
        ),
    }
}

pub fn validate_server(server: &McpServerConfig) -> Result<(), String> {
    if server.id.trim().is_empty() {
        return Err("MCP server id is required".into());
    }
    if server.command.trim().is_empty() {
        return Err(format!("MCP server '{}' command is required", server.id));
    }
    Ok(())
}

fn resolve_working_dir(server: &McpServerConfig, equipment_dir: &Path) -> PathBuf {
    server
        .working_dir
        .as_ref()
        .map(|dir| equipment_dir.join(dir))
        .unwrap_or_else(|| equipment_dir.to_path_buf())
}

async fn list_tools_once(
    server: &McpServerConfig,
    equipment_dir: &Path,
) -> anyhow::Result<Vec<McpToolDefinition>> {
    let mut session = McpStdioSession::spawn(server, equipment_dir)?;
    session.initialize().await?;
    let response = session.request("tools/list", serde_json::json!({})).await?;
    session.shutdown().await;

    let tools = response
        .get("result")
        .and_then(|result| result.get("tools"))
        .and_then(|tools| tools.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|tool| parse_tool_definition(&server.id, tool))
        .collect();
    Ok(tools)
}

async fn call_tool_once(
    server: &McpServerConfig,
    equipment_dir: &Path,
    tool_name: &str,
    arguments: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let mut session = McpStdioSession::spawn(server, equipment_dir)?;
    session.initialize().await?;
    let response = session
        .request(
            "tools/call",
            serde_json::json!({
                "name": tool_name,
                "arguments": arguments,
            }),
        )
        .await?;
    session.shutdown().await;

    if let Some(error) = response.get("error") {
        return Err(anyhow::anyhow!("MCP tool error: {error}"));
    }
    Ok(response
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

fn parse_tool_definition(server_id: &str, value: serde_json::Value) -> Option<McpToolDefinition> {
    let name = value.get("name")?.as_str()?.to_string();
    let description = value
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let input_schema = value
        .get("inputSchema")
        .or_else(|| value.get("input_schema"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({ "type": "object" }));
    Some(McpToolDefinition {
        server_id: server_id.to_string(),
        name,
        description,
        input_schema,
    })
}

struct McpStdioSession {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
    next_id: u64,
}

impl McpStdioSession {
    fn spawn(server: &McpServerConfig, equipment_dir: &Path) -> anyhow::Result<Self> {
        let mut command = Command::new(&server.command);
        command
            .args(&server.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .current_dir(resolve_working_dir(server, equipment_dir));
        for (key, value) in &server.env {
            command.env(key, value);
        }

        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("MCP server stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("MCP server stdout unavailable"))?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        let _ = self
            .request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "zhongshu", "version": env!("CARGO_PKG_VERSION") }
                }),
            )
            .await?;
        self.notify("notifications/initialized", serde_json::json!({}))
            .await
    }

    async fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_message(&request).await?;
        self.read_response(id).await
    }

    async fn notify(&mut self, method: &str, params: serde_json::Value) -> anyhow::Result<()> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_message(&notification).await
    }

    async fn write_message(&mut self, value: &serde_json::Value) -> anyhow::Result<()> {
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        self.stdin.write_all(&line).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn read_response(&mut self, id: u64) -> anyhow::Result<serde_json::Value> {
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = self.stdout.read_line(&mut line).await?;
            if bytes == 0 {
                return Err(anyhow::anyhow!("MCP server closed stdout"));
            }
            let response: serde_json::Value = serde_json::from_str(line.trim())?;
            if response.get("id").and_then(|value| value.as_u64()) == Some(id) {
                return Ok(response);
            }
        }
    }

    async fn shutdown(&mut self) {
        let _ = self.notify("shutdown", serde_json::json!({})).await;
        let _ = self.child.kill().await;
    }
}

pub fn build_mcp_tools(
    server: &McpServerConfig,
    equipment_dir: &Path,
    definitions: Vec<McpToolDefinition>,
) -> Vec<Arc<dyn Tool>> {
    definitions
        .into_iter()
        .map(|definition| {
            Arc::new(McpStdioTool::new(
                server.clone(),
                definition,
                equipment_dir.to_path_buf(),
            )) as Arc<dyn Tool>
        })
        .collect()
}

pub fn empty_env() -> BTreeMap<String, String> {
    BTreeMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_required_server_fields() {
        let server = McpServerConfig {
            id: String::new(),
            command: "node".into(),
            args: Vec::new(),
            env: empty_env(),
            working_dir: None,
            timeout_ms: 100,
        };

        assert!(validate_server(&server).is_err());
    }

    #[test]
    fn parses_mcp_tool_definition_into_tool_spec() {
        let definition = parse_tool_definition(
            "server",
            serde_json::json!({
                "name": "repo.read",
                "description": "read repo data",
                "inputSchema": { "type": "object", "properties": { "path": { "type": "string" } } }
            }),
        )
        .expect("definition");

        assert_eq!(definition.server_id, "server");
        assert_eq!(definition.name, "repo.read");
        assert_eq!(definition.spec().workspace_scope, WorkspaceScope::External);
        assert!(definition.spec().requires_approval);
    }

    #[tokio::test]
    async fn preflight_failure_is_reported_as_data() {
        let server = McpServerConfig {
            id: "missing".into(),
            command: "definitely-missing-mcp-server-binary".into(),
            args: Vec::new(),
            env: empty_env(),
            working_dir: None,
            timeout_ms: 100,
        };

        let report = preflight_stdio_server(&server, Path::new(".")).await;

        assert_eq!(report.server_id, "missing");
        assert!(report.tools.is_empty());
        assert!(report.error.is_some());
    }
}
