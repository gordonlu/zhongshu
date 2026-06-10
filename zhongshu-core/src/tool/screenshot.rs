use crate::authority::{self, CheckResult};
use crate::tool::{Tool, ToolOutput};
use async_trait::async_trait;
use serde_json::json;

pub struct ScreenshotTool;

#[async_trait]
impl Tool for ScreenshotTool {
    fn name(&self) -> &str { "screenshot" }
    fn description(&self) -> &str { "Capture a screenshot of the primary monitor as base64 PNG." }

    fn parameters(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
        match authority::check_tool("screenshot") {
            CheckResult::Deny { reason } => return ToolOutput::error(format!("[BLOCKED] {reason}")),
            CheckResult::RequireAuth { request } => {
                authority::set_pending(&request.tool, &request.program);
                return ToolOutput::auth_required(&request.program, &request.command);
            }
            CheckResult::Allow => {}
        }

        let monitors = match xcap::Monitor::all() {
            Ok(m) => m,
            Err(e) => return ToolOutput::error(format!("枚举显示器失败: {e}")),
        };

        let monitor = match monitors.into_iter().next() {
            Some(m) => m,
            None => return ToolOutput::error("未找到显示器"),
        };

        let image = match monitor.capture_image() {
            Ok(img) => img,
            Err(e) => return ToolOutput::error(format!("截图失败: {e}")),
        };

        let mut png_bytes: Vec<u8> = Vec::new();
        if let Err(e) = image.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png) {
            return ToolOutput::error(format!("PNG 编码失败: {e}"));
        }

        let b64 = base64(&png_bytes);

        ToolOutput::success(json!({
            "format": "png",
            "width": image.width(),
            "height": image.height(),
            "data": b64,
        }))
    }
}

fn base64(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((triple >> 18) & 0x3f) as usize] as char);
        out.push(CHARS[((triple >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 { CHARS[((triple >> 6) & 0x3f) as usize] } else { b'=' } as char);
        out.push(if chunk.len() > 2 { CHARS[(triple & 0x3f) as usize] } else { b'=' } as char);
    }
    out
}
