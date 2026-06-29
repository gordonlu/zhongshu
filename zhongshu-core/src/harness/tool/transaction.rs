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

pub fn capture_all_diff(workspace_root: &Path) -> Option<String> {
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

pub fn capture_diff_for_path(workspace_root: &Path, path: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["diff", "--no-color", "--", &path.to_string_lossy()])
        .current_dir(workspace_root)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        None
    }
}

pub fn safe_capture_diff(workspace_root: &Path, path: &Path) -> String {
    let diff = match capture_diff_for_path(workspace_root, path) {
        Some(d) => d,
        None => return "<binary>".to_string(),
    };
    if diff.len() > 256 * 1024 {
        return "<diff too large>".to_string();
    }
    if diff.is_empty() {
        return "<new file>".to_string();
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn init_repo(dir: &std::path::Path) {
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .output().unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test"])
            .current_dir(dir)
            .output().unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "test"])
            .current_dir(dir)
            .output().unwrap();
    }

    #[test]
    fn capture_diff_tracked_file() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();

        init_repo(&repo);

        fs::write(repo.join("test.rs"), "fn old() {}").unwrap();
        std::process::Command::new("git")
            .args(["add", "test.rs"])
            .current_dir(&repo)
            .output().unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&repo)
            .output().unwrap();

        fs::write(repo.join("test.rs"), "fn new() {}").unwrap();

        let diff = safe_capture_diff(&repo, &Path::new("test.rs"));
        assert!(diff.contains("-fn old()"), "diff should show removal of old: {diff}");
        assert!(diff.contains("+fn new()"), "diff should show addition of new: {diff}");
    }

    #[test]
    fn capture_diff_new_file() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();

        init_repo(&repo);

        fs::write(repo.join("untracked.rs"), "content").unwrap();

        let result = safe_capture_diff(&repo, &Path::new("untracked.rs"));
        assert_eq!(result, "<new file>");
    }
}
