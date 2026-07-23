use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use crate::patch::{PatchOperation, WholeFileRequest};
use crate::tool::{
    SideEffect, Tool, ToolEffect, ToolOutput, ToolRegistry, ToolSpec, WorkspaceScope,
};

const MAX_SANDBOX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SANDBOX_FILES: usize = 4096;
const MAX_SEARCH_RESULTS: usize = 100;

#[derive(Clone)]
pub struct WorkerSandbox {
    inner: Arc<WorkerSandboxInner>,
}

struct WorkerSandboxInner {
    root: PathBuf,
    exact_files: Vec<PathBuf>,
    allowed_dirs: Vec<PathBuf>,
    initial: BTreeMap<PathBuf, Option<String>>,
    sealed: AtomicBool,
}

impl Drop for WorkerSandboxInner {
    fn drop(&mut self) {
        // Normal completion uses `cleanup`, which reports failures. This
        // best-effort fallback covers cancellation and dropped worker futures.
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct SandboxRootGuard {
    root: PathBuf,
    armed: bool,
}

impl Drop for SandboxRootGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

impl WorkerSandbox {
    pub fn create(
        workspace_root: &Path,
        worker: &str,
        owned_paths: &[PathBuf],
    ) -> anyhow::Result<Self> {
        if owned_paths.is_empty() {
            anyhow::bail!("isolated sandbox requires at least one owned path");
        }
        let workspace_root = workspace_root.canonicalize()?;
        let safe_worker = worker
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let root = std::env::temp_dir()
            .join("zhongshu-agent-sandboxes")
            .join(format!("{}-{safe_worker}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;
        let mut root_guard = SandboxRootGuard {
            root: root.clone(),
            armed: true,
        };

        let mut exact_files = Vec::new();
        let mut allowed_dirs = Vec::new();
        let mut initial = BTreeMap::new();
        let mut file_count = 0usize;
        let mut total_bytes = 0u64;
        for owned in owned_paths {
            validate_relative(owned)?;
            let source = workspace_root.join(owned);
            if source.is_symlink() {
                anyhow::bail!("sandbox scope '{}' is a symlink", owned.display());
            }
            if source.is_dir() {
                allowed_dirs.push(owned.clone());
            } else {
                exact_files.push(owned.clone());
            }
        }
        copy_workspace(
            &workspace_root,
            &root,
            &mut initial,
            &mut file_count,
            &mut total_bytes,
        )?;
        for path in &exact_files {
            if workspace_root.join(path).exists() && !initial.contains_key(path) {
                anyhow::bail!(
                    "sandbox-owned file '{}' is not a regular UTF-8 text file",
                    path.display()
                );
            }
            initial.entry(path.clone()).or_insert(None);
        }

        let sandbox = Self {
            inner: Arc::new(WorkerSandboxInner {
                root,
                exact_files,
                allowed_dirs,
                initial,
                sealed: AtomicBool::new(false),
            }),
        };
        root_guard.armed = false;
        Ok(sandbox)
    }

    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    pub fn register_tools(&self, registry: ToolRegistry) -> ToolRegistry {
        registry
            .register(SandboxReadFileTool(self.clone()))
            .register(SandboxWriteFileTool(self.clone()))
            .register(SandboxEditTool(self.clone()))
            .register(SandboxListDirTool(self.clone()))
            .register(SandboxGlobTool(self.clone()))
            .register(SandboxGrepTool(self.clone()))
            .register(SandboxSearchFilesTool(self.clone()))
            .register(SandboxShellTool(self.clone()))
    }

    pub fn collect_operations_and_seal(&self) -> anyhow::Result<Vec<PatchOperation>> {
        if self.inner.sealed.load(Ordering::Acquire) {
            anyhow::bail!("sandbox changes were already submitted");
        }
        let current = self.current_files()?;
        for (path, before) in &self.inner.initial {
            if self.is_owned(path) && before.is_some() && !current.contains_key(path) {
                anyhow::bail!(
                    "sandbox deletion is not supported for owned file '{}'",
                    path.display()
                );
            }
            if self.is_owned(path) {
                continue;
            }
            match (before, current.get(path)) {
                (Some(before), Some(after)) if before == after => {}
                _ => anyhow::bail!(
                    "sandbox changed read-only context file '{}' outside its owned scope",
                    path.display()
                ),
            }
        }
        if let Some(path) = current
            .keys()
            .find(|path| !self.inner.initial.contains_key(*path) && !self.is_owned(path))
        {
            anyhow::bail!(
                "sandbox created file '{}' outside its owned scope",
                path.display()
            );
        }
        let mut operations = Vec::new();
        for (path, content) in current {
            if !self.is_owned(&path) {
                continue;
            }
            match self.inner.initial.get(&path) {
                Some(Some(before)) if before == &content => {}
                Some(Some(_)) => operations.push(PatchOperation::WriteFile(
                    WholeFileRequest::replace(path, content),
                )),
                Some(None) | None => operations.push(PatchOperation::WriteFile(
                    WholeFileRequest::create(path, content),
                )),
            }
        }
        if operations.is_empty() {
            anyhow::bail!("sandbox contains no changed files");
        }
        self.inner
            .sealed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| anyhow::anyhow!("sandbox changes were already submitted"))?;
        Ok(operations)
    }

    pub fn cleanup(&self) -> anyhow::Result<()> {
        if self.inner.root.exists() {
            fs::remove_dir_all(&self.inner.root)?;
        }
        Ok(())
    }

    fn resolve(&self, path: &Path, write: bool) -> anyhow::Result<PathBuf> {
        validate_relative(path)?;
        if write && !self.is_owned(path) {
            anyhow::bail!("path '{}' is outside the sandbox scope", path.display());
        }
        if write && self.inner.sealed.load(Ordering::Acquire) {
            anyhow::bail!("sandbox is sealed after proposal submission");
        }
        Ok(self.inner.root.join(path))
    }

    fn is_owned(&self, path: &Path) -> bool {
        self.inner.exact_files.iter().any(|owned| owned == path)
            || self
                .inner
                .allowed_dirs
                .iter()
                .any(|directory| path.starts_with(directory))
    }

    fn current_files(&self) -> anyhow::Result<BTreeMap<PathBuf, String>> {
        let mut files = BTreeMap::new();
        let mut count = 0usize;
        let mut bytes = 0u64;
        collect_directory(
            &self.inner.root,
            Path::new(""),
            &mut files,
            &mut count,
            &mut bytes,
            |path| self.is_owned(path),
        )?;
        enforce_limits(count, bytes)?;
        Ok(files)
    }
}

fn validate_relative(path: &Path) -> anyhow::Result<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        anyhow::bail!("sandbox paths must be non-empty workspace-relative paths without '..'");
    }
    Ok(())
}

fn enforce_limits(files: usize, bytes: u64) -> anyhow::Result<()> {
    if files > MAX_SANDBOX_FILES {
        anyhow::bail!("sandbox exceeds {MAX_SANDBOX_FILES} files");
    }
    if bytes > MAX_SANDBOX_BYTES {
        anyhow::bail!("sandbox exceeds {MAX_SANDBOX_BYTES} bytes");
    }
    Ok(())
}

fn ignored_entry(entry: &walkdir::DirEntry) -> bool {
    matches!(
        entry.file_name().to_str(),
        Some(".git" | ".roadmap" | "target" | "node_modules" | ".next")
    )
}

fn copy_workspace(
    workspace_root: &Path,
    sandbox_root: &Path,
    initial: &mut BTreeMap<PathBuf, Option<String>>,
    file_count: &mut usize,
    total_bytes: &mut u64,
) -> anyhow::Result<()> {
    for entry in walkdir::WalkDir::new(workspace_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !ignored_entry(entry))
    {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let path = entry.path().strip_prefix(workspace_root)?.to_path_buf();
        let bytes = fs::read(entry.path())?;
        if bytes.contains(&0) {
            continue;
        }
        let Ok(content) = String::from_utf8(bytes) else {
            continue;
        };
        *total_bytes = total_bytes.saturating_add(content.len() as u64);
        *file_count += 1;
        enforce_limits(*file_count, *total_bytes)?;
        let destination = sandbox_root.join(&path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(destination, &content)?;
        initial.insert(path, Some(content));
    }
    Ok(())
}

fn collect_directory(
    sandbox_root: &Path,
    relative: &Path,
    files: &mut BTreeMap<PathBuf, String>,
    count: &mut usize,
    bytes: &mut u64,
    strict_binary: impl Fn(&Path) -> bool,
) -> anyhow::Result<()> {
    let absolute = sandbox_root.join(relative);
    if !absolute.exists() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(&absolute)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !ignored_entry(entry))
    {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.file_type().is_symlink() {
            anyhow::bail!("sandbox contains a symlink: {}", entry.path().display());
        }
        if metadata.is_file() {
            let path = entry.path().strip_prefix(sandbox_root)?.to_path_buf();
            let raw = fs::read(entry.path())?;
            let content = if raw.contains(&0) {
                None
            } else {
                String::from_utf8(raw).ok()
            };
            let Some(content) = content else {
                if strict_binary(&path) {
                    anyhow::bail!("sandbox-owned file '{}' is not UTF-8 text", path.display());
                }
                continue;
            };
            *bytes = bytes.saturating_add(content.len() as u64);
            files.insert(path, content);
            *count += 1;
            enforce_limits(*count, *bytes)?;
        }
    }
    Ok(())
}

fn sandbox_path(arguments: &serde_json::Value) -> anyhow::Result<PathBuf> {
    arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("'path' must be a string"))
}

fn sandbox_search_root(
    sandbox: &WorkerSandbox,
    arguments: &serde_json::Value,
) -> anyhow::Result<PathBuf> {
    let path = arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.is_empty())
        .unwrap_or(".");
    let resolved = sandbox.resolve(Path::new(path), false)?;
    if !resolved.exists() {
        anyhow::bail!("sandbox search path '{path}' does not exist");
    }
    Ok(resolved)
}

fn file_spec(name: &str, write: bool) -> ToolSpec {
    ToolSpec::new(name)
        .with_effect(if write {
            ToolEffect::Write
        } else {
            ToolEffect::Read
        })
        .read_only(!write)
        .workspace_scope(WorkspaceScope::WorkspaceOnly)
        .requires_approval(false)
        .side_effect(if write {
            SideEffect::LocalWrite
        } else {
            SideEffect::ReadOnly
        })
}

struct SandboxReadFileTool(WorkerSandbox);
struct SandboxWriteFileTool(WorkerSandbox);
struct SandboxEditTool(WorkerSandbox);
struct SandboxListDirTool(WorkerSandbox);
struct SandboxGlobTool(WorkerSandbox);
struct SandboxGrepTool(WorkerSandbox);
struct SandboxSearchFilesTool(WorkerSandbox);
struct SandboxShellTool(WorkerSandbox);

#[async_trait]
impl Tool for SandboxReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "Read a workspace-relative text file from this employee's isolated sandbox."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})
    }
    fn spec(&self) -> ToolSpec {
        file_spec(self.name(), false)
    }
    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let result = sandbox_path(arguments)
            .and_then(|path| self.0.resolve(&path, false))
            .and_then(|path| fs::read_to_string(path).map_err(Into::into));
        match result {
            Ok(content) => ToolOutput::success(serde_json::json!({"content":content})),
            Err(error) => ToolOutput::error(error.to_string()),
        }
    }
}

#[async_trait]
impl Tool for SandboxWriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }
    fn description(&self) -> &str {
        "Write a workspace-relative text file inside this employee's isolated sandbox only."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]})
    }
    fn spec(&self) -> ToolSpec {
        file_spec(self.name(), true)
    }
    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let content = match arguments.get("content").and_then(serde_json::Value::as_str) {
            Some(content) => content,
            None => return ToolOutput::error("'content' must be a string"),
        };
        let result = sandbox_path(arguments).and_then(|relative| {
            let absolute = self.0.resolve(&relative, true)?;
            if let Some(parent) = absolute.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&absolute, content)?;
            Ok(relative)
        });
        match result {
            Ok(path) => ToolOutput::success(serde_json::json!({"path":path,"written":true})),
            Err(error) => ToolOutput::error(error.to_string()),
        }
    }
}

#[async_trait]
impl Tool for SandboxEditTool {
    fn name(&self) -> &str {
        "edit"
    }
    fn description(&self) -> &str {
        "Replace text in a workspace-relative file inside this employee's isolated sandbox only."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"old":{"type":"string"},"new":{"type":"string"}},"required":["path","old","new"]})
    }
    fn spec(&self) -> ToolSpec {
        file_spec(self.name(), true)
    }
    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let old = match arguments.get("old").and_then(serde_json::Value::as_str) {
            Some(value) => value,
            None => return ToolOutput::error("'old' must be a string"),
        };
        let new = arguments
            .get("new")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let result = sandbox_path(arguments).and_then(|relative| {
            let absolute = self.0.resolve(&relative, true)?;
            let content = fs::read_to_string(&absolute)?;
            if !content.contains(old) {
                anyhow::bail!("old text was not found");
            }
            fs::write(&absolute, content.replacen(old, new, 1))?;
            Ok(relative)
        });
        match result {
            Ok(path) => ToolOutput::success(serde_json::json!({"path":path,"replaced":1})),
            Err(error) => ToolOutput::error(error.to_string()),
        }
    }
}

#[async_trait]
impl Tool for SandboxListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }
    fn description(&self) -> &str {
        "List a workspace-relative directory inside this employee's isolated sandbox."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})
    }
    fn spec(&self) -> ToolSpec {
        file_spec(self.name(), false)
    }
    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let result = sandbox_path(arguments)
            .and_then(|path| self.0.resolve(&path, false))
            .and_then(|path| {
                let entries = fs::read_dir(path)?
                    .map(|entry| {
                        entry.map(|entry| entry.file_name().to_string_lossy().into_owned())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(entries)
            });
        match result {
            Ok(entries) => ToolOutput::success(serde_json::json!({"entries":entries})),
            Err(error) => ToolOutput::error(error.to_string()),
        }
    }
}

#[async_trait]
impl Tool for SandboxGlobTool {
    fn name(&self) -> &str {
        "glob"
    }
    fn description(&self) -> &str {
        "Find files by glob pattern inside this employee's isolated sandbox."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string","default":"."}},"required":["pattern"]})
    }
    fn spec(&self) -> ToolSpec {
        file_spec(self.name(), false)
    }
    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let Some(pattern) = arguments
            .get("pattern")
            .and_then(serde_json::Value::as_str)
            .filter(|pattern| !pattern.is_empty())
        else {
            return ToolOutput::error("'pattern' must be a non-empty string");
        };
        let result = (|| -> anyhow::Result<Vec<String>> {
            let search_root = sandbox_search_root(&self.0, arguments)?;
            let matcher = globset::Glob::new(pattern)?.compile_matcher();
            let mut matches = Vec::new();
            for entry in walkdir::WalkDir::new(&search_root).follow_links(false) {
                let entry = entry?;
                if !entry.file_type().is_file() {
                    continue;
                }
                let relative_to_search = entry.path().strip_prefix(&search_root)?;
                if matcher.is_match(relative_to_search) {
                    matches.push(
                        entry
                            .path()
                            .strip_prefix(self.0.root())?
                            .to_string_lossy()
                            .into_owned(),
                    );
                    if matches.len() == MAX_SEARCH_RESULTS {
                        break;
                    }
                }
            }
            matches.sort();
            Ok(matches)
        })();
        match result {
            Ok(paths) => ToolOutput::success(serde_json::json!({"paths":paths})),
            Err(error) => ToolOutput::error(error.to_string()),
        }
    }
}

#[async_trait]
impl Tool for SandboxGrepTool {
    fn name(&self) -> &str {
        "grep"
    }
    fn description(&self) -> &str {
        "Search UTF-8 file contents for a literal string inside this employee's isolated sandbox."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"pattern":{"type":"string","description":"Literal text to find"},"path":{"type":"string","default":"."}},"required":["pattern"]})
    }
    fn spec(&self) -> ToolSpec {
        file_spec(self.name(), false)
    }
    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let Some(pattern) = arguments
            .get("pattern")
            .and_then(serde_json::Value::as_str)
            .filter(|pattern| !pattern.is_empty())
        else {
            return ToolOutput::error("'pattern' must be a non-empty string");
        };
        let result = (|| -> anyhow::Result<Vec<serde_json::Value>> {
            let search_root = sandbox_search_root(&self.0, arguments)?;
            let mut matches = Vec::new();
            for entry in walkdir::WalkDir::new(&search_root).follow_links(false) {
                let entry = entry?;
                if !entry.file_type().is_file() {
                    continue;
                }
                let Ok(content) = fs::read_to_string(entry.path()) else {
                    continue;
                };
                for (line_index, line) in content.lines().enumerate() {
                    if line.contains(pattern) {
                        matches.push(serde_json::json!({
                            "path": entry.path().strip_prefix(self.0.root())?.to_string_lossy(),
                            "line": line_index + 1,
                            "text": line,
                        }));
                        if matches.len() == MAX_SEARCH_RESULTS {
                            return Ok(matches);
                        }
                    }
                }
            }
            Ok(matches)
        })();
        match result {
            Ok(matches) => ToolOutput::success(serde_json::json!({"matches":matches})),
            Err(error) => ToolOutput::error(error.to_string()),
        }
    }
}

#[async_trait]
impl Tool for SandboxSearchFilesTool {
    fn name(&self) -> &str {
        "search_files"
    }
    fn description(&self) -> &str {
        "Search sandbox file paths by case-insensitive substring."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"query":{"type":"string"},"path":{"type":"string","default":"."}},"required":["query"]})
    }
    fn spec(&self) -> ToolSpec {
        file_spec(self.name(), false)
    }
    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let Some(query) = arguments
            .get("query")
            .and_then(serde_json::Value::as_str)
            .filter(|query| !query.is_empty())
        else {
            return ToolOutput::error("'query' must be a non-empty string");
        };
        let query = query.to_lowercase();
        let result = (|| -> anyhow::Result<Vec<String>> {
            let search_root = sandbox_search_root(&self.0, arguments)?;
            let mut matches = Vec::new();
            for entry in walkdir::WalkDir::new(&search_root).follow_links(false) {
                let entry = entry?;
                if !entry.file_type().is_file() {
                    continue;
                }
                let relative = entry.path().strip_prefix(self.0.root())?;
                if relative.to_string_lossy().to_lowercase().contains(&query) {
                    matches.push(relative.to_string_lossy().into_owned());
                    if matches.len() == MAX_SEARCH_RESULTS {
                        break;
                    }
                }
            }
            matches.sort();
            Ok(matches)
        })();
        match result {
            Ok(paths) => ToolOutput::success(serde_json::json!({"paths":paths})),
            Err(error) => ToolOutput::error(error.to_string()),
        }
    }
}

#[async_trait]
impl Tool for SandboxShellTool {
    fn name(&self) -> &str {
        "shell"
    }
    fn description(&self) -> &str {
        "Run a build or verification command from /workspace inside this employee's isolated sandbox. Use workspace-relative paths; host sandbox paths are unavailable. Network, the user workspace, and host write paths are unavailable."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]})
    }
    fn spec(&self) -> ToolSpec {
        file_spec(self.name(), true)
    }
    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let Some(command) = arguments.get("command").and_then(serde_json::Value::as_str) else {
            return ToolOutput::error("'command' must be a string");
        };
        #[cfg(not(target_os = "linux"))]
        {
            let _ = command;
            return ToolOutput::error("isolated sandbox shell currently requires Linux bubblewrap");
        }
        #[cfg(target_os = "linux")]
        {
            if !Path::new("/usr/bin/bwrap").exists() {
                return ToolOutput::error("isolated sandbox shell requires /usr/bin/bwrap");
            }
            let mut process = tokio::process::Command::new("/usr/bin/bwrap");
            process.kill_on_drop(true);
            process.args([
                "--die-with-parent",
                "--new-session",
                "--unshare-all",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--tmpfs",
                "/tmp",
                "--ro-bind",
                "/usr",
                "/usr",
            ]);
            // Mount /bin only if it's a real directory (not a symlink to /usr/bin).
            // On modern Ubuntu /bin -> usr/bin, and bwrap --ro-bind follows
            // symlinks, causing the mount to fail.
            if std::fs::metadata("/bin")
                .map(|m| m.is_dir())
                .unwrap_or(false)
            {
                process.args(["--ro-bind", "/bin", "/bin"]);
            }
            process.args(["--ro-bind", "/etc", "/etc"]);
            process.arg(if self.0.inner.sealed.load(Ordering::Acquire) {
                "--ro-bind"
            } else {
                "--bind"
            });
            process.arg(self.0.root()).arg("/workspace");
            for path in ["/lib", "/lib64"] {
                if Path::new(path).exists() {
                    process.args(["--ro-bind", path, path]);
                }
            }
            for (variable, fallback) in [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")] {
                let path = std::env::var_os(variable).map(PathBuf::from).or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(fallback))
                });
                if let Some(path) = path.filter(|path| path.exists()) {
                    process.arg("--ro-bind").arg(&path).arg(&path);
                    process.arg("--setenv").arg(variable).arg(&path);
                }
            }
            process.args([
                "--setenv",
                "HOME",
                "/tmp",
                "--chdir",
                "/workspace",
                "--",
                "/usr/bin/sh",
                "-lc",
                command,
            ]);
            let output = match process.output().await {
                Ok(output) => output,
                Err(error) => return ToolOutput::error(format!("sandbox command failed: {error}")),
            };
            let exit_code = output.status.code().unwrap_or(-1);
            let data = serde_json::json!({
                "exit_code": exit_code,
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
                "sandboxed": true,
                "network": "disabled"
            });
            if output.status.success() {
                ToolOutput::success(data)
            } else {
                ToolOutput::error(data.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sandbox_writes_are_isolated_scoped_and_convert_to_patch_operations() {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join("src")).unwrap();
        fs::write(workspace.path().join("src/a.txt"), "before").unwrap();
        fs::write(workspace.path().join("context.txt"), "read only").unwrap();
        let sandbox =
            WorkerSandbox::create(workspace.path(), "writer", &[PathBuf::from("src")]).unwrap();
        let registry = sandbox.register_tools(ToolRegistry::new());

        assert_eq!(
            registry
                .execute("read_file", r#"{"path":"context.txt"}"#)
                .await
                .status,
            crate::tool::ToolStatus::Success
        );
        let write = registry
            .execute("write_file", r#"{"path":"src/a.txt","content":"after"}"#)
            .await;
        assert_eq!(write.status, crate::tool::ToolStatus::Success);
        assert_eq!(write.data.unwrap()["path"], "src/a.txt");
        let edit = registry
            .execute(
                "edit",
                r#"{"path":"src/a.txt","old":"after","new":"edited"}"#,
            )
            .await;
        assert_eq!(edit.status, crate::tool::ToolStatus::Success);
        assert_eq!(edit.data.unwrap()["path"], "src/a.txt");
        assert_eq!(
            registry
                .execute("write_file", r#"{"path":"../escape","content":"bad"}"#)
                .await
                .status,
            crate::tool::ToolStatus::Error
        );
        assert_eq!(
            registry
                .execute(
                    "shell",
                    r#"{"command":"test \"$(cat src/a.txt)\" = edited"}"#,
                )
                .await
                .status,
            crate::tool::ToolStatus::Success
        );
        assert_eq!(
            fs::read_to_string(workspace.path().join("src/a.txt")).unwrap(),
            "before"
        );
        let operations = sandbox.collect_operations_and_seal().unwrap();
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].path(), Path::new("src/a.txt"));
        assert_eq!(
            registry
                .execute("write_file", r#"{"path":"src/a.txt","content":"later"}"#)
                .await
                .status,
            crate::tool::ToolStatus::Error
        );
        sandbox.cleanup().unwrap();
        assert!(!sandbox.root().exists());
    }

    #[tokio::test]
    async fn proposal_rejects_shell_changes_to_read_only_context() {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join("src")).unwrap();
        fs::write(workspace.path().join("src/a.txt"), "before").unwrap();
        fs::write(workspace.path().join("project.conf"), "original").unwrap();
        let sandbox =
            WorkerSandbox::create(workspace.path(), "writer", &[PathBuf::from("src")]).unwrap();
        let registry = sandbox.register_tools(ToolRegistry::new());

        assert_eq!(
            registry
                .execute(
                    "shell",
                    r#"{"command":"printf changed > project.conf; printf after > src/a.txt"}"#,
                )
                .await
                .status,
            crate::tool::ToolStatus::Success
        );
        assert!(sandbox
            .collect_operations_and_seal()
            .unwrap_err()
            .to_string()
            .contains("outside its owned scope"));
        assert_eq!(
            fs::read_to_string(workspace.path().join("project.conf")).unwrap(),
            "original"
        );
    }

    #[tokio::test]
    async fn sandbox_discovery_tools_only_read_the_isolated_copy() {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join("artifacts/proof-runs/sandbox-canary")).unwrap();
        fs::write(
            workspace
                .path()
                .join("artifacts/proof-runs/sandbox-canary/status.txt"),
            "status=pending\n",
        )
        .unwrap();
        let sandbox = WorkerSandbox::create(
            workspace.path(),
            "discoverer",
            &[PathBuf::from("artifacts/proof-runs/sandbox-canary")],
        )
        .unwrap();
        let registry = sandbox.register_tools(ToolRegistry::new());

        fs::write(
            workspace
                .path()
                .join("artifacts/proof-runs/sandbox-canary/status.txt"),
            "host changed after sandbox creation\n",
        )
        .unwrap();

        let glob = registry
            .execute("glob", r#"{"pattern":"**/status.txt"}"#)
            .await;
        assert_eq!(glob.status, crate::tool::ToolStatus::Success);
        assert!(glob.data.unwrap()["paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path.as_str().unwrap().ends_with("status.txt")));

        let search = registry
            .execute("search_files", r#"{"query":"STATUS.TXT"}"#)
            .await;
        assert_eq!(search.status, crate::tool::ToolStatus::Success);
        assert_eq!(search.data.unwrap()["paths"].as_array().unwrap().len(), 1);

        let grep = registry
            .execute("grep", r#"{"pattern":"status=pending","path":"artifacts"}"#)
            .await;
        assert_eq!(grep.status, crate::tool::ToolStatus::Success);
        let matches = grep.data.unwrap();
        assert_eq!(matches["matches"].as_array().unwrap().len(), 1);
        assert_eq!(
            fs::read_to_string(
                sandbox
                    .root()
                    .join("artifacts/proof-runs/sandbox-canary/status.txt")
            )
            .unwrap(),
            "status=pending\n"
        );
    }

    #[test]
    fn dropped_sandbox_removes_worker_directory() {
        let workspace = tempfile::tempdir().unwrap();
        fs::write(workspace.path().join("owned.txt"), "content").unwrap();
        let sandbox =
            WorkerSandbox::create(workspace.path(), "cancelled", &[PathBuf::from("owned.txt")])
                .unwrap();
        let root = sandbox.root().to_path_buf();

        drop(sandbox);

        assert!(!root.exists());
    }
}
