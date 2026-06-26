use std::path::Path;

/// Check whether the workspace has uncommitted changes (working tree
/// or staged). Returns true if git is available and reports changes.
pub fn workspace_has_mutations(workspace_root: &Path) -> bool {
    std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(workspace_root)
        .output()
        .ok()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

pub fn capture_diff(workspace_root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["diff", "--no-color"])
        .current_dir(workspace_root)
        .output()
        .ok()?;
    if output.status.success() {
        let diff = String::from_utf8_lossy(&output.stdout).to_string();
        if diff.is_empty() {
            None
        } else {
            Some(diff)
        }
    } else {
        None
    }
}
