use std::path::Path;

pub fn capture_diff(workspace_root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["diff", "--no-color"])
        .current_dir(workspace_root)
        .output()
        .ok()?;
    if output.status.success() {
        let diff = String::from_utf8_lossy(&output.stdout).to_string();
        if diff.is_empty() { None } else { Some(diff) }
    } else {
        None
    }
}
