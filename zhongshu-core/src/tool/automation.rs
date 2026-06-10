use crate::authority::{self, CheckResult};
use crate::tool::{Tool, ToolOutput};
use async_trait::async_trait;
use serde_json::json;
use tracing::info;

pub struct AutomationTool;

#[async_trait]
impl Tool for AutomationTool {
    fn name(&self) -> &str { "desktop" }
    fn description(&self) -> &str {
        "Desktop automation: type text, press keys, move/click mouse."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["type", "key", "click", "move"]},
                "text": {"type": "string", "description": "Text to type (for 'type' action)"},
                "keys": {"type": "string", "description": "Key combo (for 'key' action), e.g. 'ctrl+c'"},
                "x": {"type": "integer", "description": "X coordinate"},
                "y": {"type": "integer", "description": "Y coordinate"}
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        match authority::check_tool("automation") {
            CheckResult::Deny { reason } => return ToolOutput::error(format!("[BLOCKED] {reason}")),
            CheckResult::RequireAuth { request } => {
                authority::set_pending(&request.tool, &request.program);
                return ToolOutput::auth_required(&request.program, &request.command);
            }
            CheckResult::Allow => {}
        }

        let action = match arguments["action"].as_str() {
            Some(a) => a,
            None => return ToolOutput::error("'action' must be a string"),
        };

        #[cfg(any(target_os = "linux", target_os = "windows"))]
        { return exec_automation(action, arguments); }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        { let _ = (action, arguments); ToolOutput::error("此平台不支持桌面自动化") }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
use {enigo::Keyboard, enigo::Mouse};

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn exec_automation(action: &str, arguments: &serde_json::Value) -> ToolOutput {
    let mut enigo = match enigo::Enigo::new(&enigo::Settings::default()) {
        Ok(e) => e,
        Err(e) => return ToolOutput::error(format!("enigo 初始化失败: {e}")),
    };

    match action {
        "type" => {
            let text = match arguments["text"].as_str() {
                Some(t) => t,
                None => return ToolOutput::error("'text' must be a string"),
            };
            info!("desktop type: {text}");
            match enigo.text(text) {
                Ok(_) => ToolOutput::success(json!({ "action": "type", "text": text })),
                Err(e) => ToolOutput::error(format!("输入失败: {e}")),
            }
        }
        "key" => {
            let keys = match arguments["keys"].as_str() {
                Some(k) => k,
                None => return ToolOutput::error("'keys' must be a string"),
            };
            info!("desktop key: {keys}");
            match sim_key_combo(&mut enigo, keys) {
                Ok(_) => ToolOutput::success(json!({ "action": "key", "keys": keys })),
                Err(e) => ToolOutput::error(format!("按键失败: {e}")),
            }
        }
        "click" => {
            let x = arguments["x"].as_i64().unwrap_or(0) as i32;
            let y = arguments["y"].as_i64().unwrap_or(0) as i32;
            if let Err(e) = enigo.move_mouse(x, y, enigo::Coordinate::Abs) {
                return ToolOutput::error(format!("移动鼠标失败: {e}"));
            }
            match enigo.button(enigo::Button::Left, enigo::Direction::Click) {
                Ok(_) => ToolOutput::success(json!({ "action": "click", "x": x, "y": y })),
                Err(e) => ToolOutput::error(format!("点击失败: {e}")),
            }
        }
        "move" => {
            let x = arguments["x"].as_i64().unwrap_or(0) as i32;
            let y = arguments["y"].as_i64().unwrap_or(0) as i32;
            match enigo.move_mouse(x, y, enigo::Coordinate::Abs) {
                Ok(_) => ToolOutput::success(json!({ "action": "move", "x": x, "y": y })),
                Err(e) => ToolOutput::error(format!("移动失败: {e}")),
            }
        }
        other => ToolOutput::error(format!("未知操作: '{other}'")),
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn sim_key_combo(enigo: &mut enigo::Enigo, combo: &str) -> anyhow::Result<()> {
    let parts: Vec<&str> = combo.split('+').map(|s| s.trim()).collect();
    let mut keys: Vec<enigo::Key> = Vec::new();
    for part in &parts { keys.push(parse_key(part)?); }
    for key in &keys { enigo.key(*key, enigo::Direction::Press).map_err(|e| anyhow::anyhow!("{e}"))?; }
    for key in keys.iter().rev() { enigo.key(*key, enigo::Direction::Release).map_err(|e| anyhow::anyhow!("{e}"))?; }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn parse_key(s: &str) -> anyhow::Result<enigo::Key> {
    match s.to_lowercase().as_str() {
        "ctrl" | "control" => Ok(enigo::Key::Control),
        "alt" => Ok(enigo::Key::Alt),
        "shift" => Ok(enigo::Key::Shift),
        "meta" | "win" | "command" | "cmd" | "super" => Ok(enigo::Key::Meta),
        "enter" | "return" => Ok(enigo::Key::Return),
        "space" => Ok(enigo::Key::Space),
        "tab" => Ok(enigo::Key::Tab),
        "escape" | "esc" => Ok(enigo::Key::Escape),
        "backspace" => Ok(enigo::Key::Backspace),
        "delete" => Ok(enigo::Key::Delete),
        "up" => Ok(enigo::Key::UpArrow),
        "down" => Ok(enigo::Key::DownArrow),
        "left" => Ok(enigo::Key::LeftArrow),
        "right" => Ok(enigo::Key::RightArrow),
        "home" => Ok(enigo::Key::Home),
        "end" => Ok(enigo::Key::End),
        "pageup" | "pgup" => Ok(enigo::Key::PageUp),
        "pagedown" | "pgdn" => Ok(enigo::Key::PageDown),
        single if single.len() == 1 => Ok(enigo::Key::Unicode(single.chars().next().unwrap())),
        _ => Err(anyhow::anyhow!("unknown key '{s}'")),
    }
}
