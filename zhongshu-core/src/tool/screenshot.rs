use crate::authority::{self, CheckResult};
use crate::tool::{Tool, ToolOutput};
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

pub struct ScreenshotTool;

fn screenshots_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/zhongshu/screenshots")
}

fn ensure_dir(path: &PathBuf) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

#[async_trait]
impl Tool for ScreenshotTool {
    fn name(&self) -> &str {
        "screenshot"
    }
    fn description(&self) -> &str {
        "Capture a screenshot of the primary monitor, save as PNG, and return the file path."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
        match authority::check_tool("screenshot") {
            CheckResult::Deny { reason } => {
                return ToolOutput::error(format!("[BLOCKED] {reason}"))
            }
            CheckResult::RequireAuth { request } => {
                authority::set_pending(&request.tool, &request.program, &request.command, "");
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
        if let Err(e) = image.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        ) {
            return ToolOutput::error(format!("PNG 编码失败: {e}"));
        }
        let file_size = png_bytes.len();

        let dir = screenshots_dir();
        if let Err(e) = ensure_dir(&dir) {
            return ToolOutput::error(format!("创建截图目录失败: {e}"));
        }

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let filename = format!("screenshot_{timestamp}.png");
        let path = dir.join(&filename);

        if let Err(e) = std::fs::write(&path, &png_bytes) {
            return ToolOutput::error(format!("保存截图失败: {e}"));
        }

        let path_str = path.to_string_lossy().into_owned();

        ToolOutput::success(json!({
            "path": path_str,
            "format": "png",
            "width": image.width(),
            "height": image.height(),
            "file_size_bytes": file_size,
        }))
        .external()
    }
}
