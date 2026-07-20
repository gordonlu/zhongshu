use std::path::{Path, PathBuf};

/// Check whether the workspace has uncommitted changes (working tree
/// or staged). Returns true if git is available and reports changes.
pub fn workspace_has_mutations(workspace_root: &Path) -> bool {
    workspace_mutation_snapshot(workspace_root).is_some_and(|snapshot| !snapshot.is_empty())
}

/// Capture the current Git working-tree mutation state so callers can compare
/// before/after a shell command. Looking only at the post-command state would
/// misattribute pre-existing user changes to a read-only command such as tests.
pub fn workspace_mutation_snapshot(workspace_root: &Path) -> Option<Vec<u8>> {
    std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(workspace_root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| output.stdout)
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
    let path = workspace_relative_path(workspace_root, path);
    let output = std::process::Command::new("git")
        .args(["diff", "--no-color", "--", &path.to_string_lossy()])
        .current_dir(workspace_root)
        .output()
        .ok()?;
    if output.status.success() {
        let diff = String::from_utf8_lossy(&output.stdout).to_string();
        if diff.is_empty() {
            capture_untracked_text_diff(workspace_root, &path)
        } else {
            Some(diff)
        }
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

fn workspace_relative_path(workspace_root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.strip_prefix(workspace_root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

fn capture_untracked_text_diff(workspace_root: &Path, path: &Path) -> Option<String> {
    let tracked = std::process::Command::new("git")
        .args(["ls-files", "--error-unmatch", "--", &path.to_string_lossy()])
        .current_dir(workspace_root)
        .output()
        .ok()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if tracked {
        return Some(String::new());
    }

    let full_path = workspace_root.join(path);
    if !full_path.is_file() {
        return None;
    }
    let content = std::fs::read(&full_path).ok()?;
    if content.contains(&0) {
        return Some("<binary>".to_string());
    }
    let content = String::from_utf8(content).ok()?;
    if content.len() > 256 * 1024 {
        return Some("<diff too large>".to_string());
    }

    let path = path.to_string_lossy().replace('\\', "/");
    let mut diff = String::new();
    diff.push_str("diff --git a/");
    diff.push_str(&path);
    diff.push_str(" b/");
    diff.push_str(&path);
    diff.push('\n');
    diff.push_str("new file mode 100644\n");
    diff.push_str("--- /dev/null\n");
    diff.push_str("+++ b/");
    diff.push_str(&path);
    diff.push('\n');
    let added_lines = content.lines().count().max(1);
    diff.push_str(&format!("@@ -0,0 +1,{added_lines} @@\n"));
    if content.is_empty() {
        diff.push_str("+\n");
    } else {
        for line in content.lines() {
            diff.push('+');
            diff.push_str(line);
            diff.push('\n');
        }
    }
    Some(diff)
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
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test"])
            .current_dir(dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "test"])
            .current_dir(dir)
            .output()
            .unwrap();
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
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&repo)
            .output()
            .unwrap();

        fs::write(repo.join("test.rs"), "fn new() {}").unwrap();

        let diff = safe_capture_diff(&repo, &Path::new("test.rs"));
        assert!(
            diff.contains("-fn old()"),
            "diff should show removal of old: {diff}"
        );
        assert!(
            diff.contains("+fn new()"),
            "diff should show addition of new: {diff}"
        );
    }

    #[test]
    fn capture_diff_new_file() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();

        init_repo(&repo);

        fs::write(repo.join("untracked.rs"), "content").unwrap();

        let result = safe_capture_diff(&repo, &Path::new("untracked.rs"));
        assert!(result.contains("--- /dev/null"), "{result}");
        assert!(result.contains("+++ b/untracked.rs"), "{result}");
        assert!(result.contains("+content"), "{result}");
    }

    #[test]
    fn capture_diff_accepts_absolute_workspace_path() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();

        init_repo(&repo);

        fs::write(repo.join("test.rs"), "fn old() {}").unwrap();
        std::process::Command::new("git")
            .args(["add", "test.rs"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&repo)
            .output()
            .unwrap();

        fs::write(repo.join("test.rs"), "fn new() {}").unwrap();

        let diff = safe_capture_diff(&repo, &repo.join("test.rs"));
        assert!(diff.contains("--- a/test.rs"), "{diff}");
        assert!(diff.contains("+fn new()"), "{diff}");
    }
}
