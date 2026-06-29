use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub struct PatchEngine {
    workspace_root: PathBuf,
    reads: BTreeMap<PathBuf, FileSnapshot>,
    max_file_bytes: u64,
}

impl PatchEngine {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Result<Self, PatchError> {
        let root = workspace_root.into();
        let root = root
            .canonicalize()
            .map_err(|e| PatchError::WorkspaceUnavailable {
                path: root.clone(),
                message: e.to_string(),
            })?;
        Ok(Self {
            workspace_root: root,
            reads: BTreeMap::new(),
            max_file_bytes: 1_073_741_824,
        })
    }

    pub fn with_max_file_bytes(mut self, max_file_bytes: u64) -> Self {
        self.max_file_bytes = max_file_bytes;
        self
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn read(&mut self, path: impl AsRef<Path>) -> Result<FileSnapshot, PatchError> {
        let path = self.resolve_workspace_path(path.as_ref())?;
        let snapshot = self.read_snapshot(&path)?;
        self.reads.insert(path, snapshot.clone());
        Ok(snapshot)
    }

    pub fn preview_replace(&self, request: ReplaceRequest) -> Result<PatchPreview, PatchError> {
        let path = self.resolve_workspace_path(&request.path)?;
        let snapshot = self
            .reads
            .get(&path)
            .ok_or_else(|| PatchError::FileNotRead { path: path.clone() })?;
        self.ensure_not_stale(snapshot)?;
        let updated = apply_replace(&snapshot.content, &request)?;
        Ok(PatchPreview::new(
            path,
            snapshot,
            updated,
            request.replace_all,
        ))
    }

    pub fn preview_multi_replace(
        &self,
        request: MultiReplaceRequest,
    ) -> Result<PatchPreview, PatchError> {
        let path = self.resolve_workspace_path(&request.path)?;
        let snapshot = self
            .reads
            .get(&path)
            .ok_or_else(|| PatchError::FileNotRead { path: path.clone() })?;
        self.ensure_not_stale(snapshot)?;
        let updated = apply_multi_replace(&snapshot.content, &request.edits)?;
        Ok(PatchPreview::new(path, snapshot, updated, true))
    }

    pub fn preview_write_file(
        &self,
        request: WholeFileRequest,
    ) -> Result<PatchPreview, PatchError> {
        let path = self.resolve_workspace_path(&request.path)?;
        match self.reads.get(&path) {
            Some(snapshot) => {
                self.ensure_not_stale(snapshot)?;
                if snapshot.content == request.content {
                    return Err(PatchError::NoOp);
                }
                Ok(PatchPreview::new(path, snapshot, request.content, true))
            }
            None if path.exists() => Err(PatchError::FileNotRead { path }),
            None if request.allow_create => {
                if let Some(parent) = path.parent() {
                    if !parent.exists() {
                        return Err(PatchError::ParentDirectoryMissing {
                            path: parent.to_path_buf(),
                        });
                    }
                }
                let line_ending = request
                    .line_ending
                    .unwrap_or_else(|| LineEnding::detect(&request.content));
                let snapshot = FileSnapshot {
                    path: path.clone(),
                    content: String::new(),
                    modified: None,
                    encoding: request.encoding.unwrap_or(TextEncoding::Utf8),
                    line_ending,
                };
                if request.content.is_empty() {
                    return Err(PatchError::NoOp);
                }
                Ok(PatchPreview::new(path, &snapshot, request.content, true))
            }
            None => Err(PatchError::FileNotRead { path }),
        }
    }

    pub fn preview_operation(
        &self,
        operation: PatchOperation,
    ) -> Result<PatchPreview, PatchAttemptFailure> {
        let kind = operation.kind();
        let path = operation.path().to_path_buf();
        let result = match operation {
            PatchOperation::Replace(request) => self.preview_replace(request),
            PatchOperation::MultiReplace(request) => self.preview_multi_replace(request),
            PatchOperation::WriteFile(request) => self.preview_write_file(request),
        };
        result.map_err(|error| PatchAttemptFailure::from_error(kind, Some(path), error))
    }

    pub fn apply_replace(&mut self, request: ReplaceRequest) -> Result<PatchResult, PatchError> {
        let preview = self.preview_replace(request)?;
        self.apply_preview(preview)
    }

    pub fn apply_multi_replace(
        &mut self,
        request: MultiReplaceRequest,
    ) -> Result<PatchResult, PatchError> {
        let preview = self.preview_multi_replace(request)?;
        self.apply_preview(preview)
    }

    pub fn apply_write_file(
        &mut self,
        request: WholeFileRequest,
    ) -> Result<PatchResult, PatchError> {
        let preview = self.preview_write_file(request)?;
        self.apply_preview(preview)
    }

    pub fn apply_operation(
        &mut self,
        operation: PatchOperation,
    ) -> Result<PatchResult, PatchAttemptFailure> {
        let kind = operation.kind();
        let path = operation.path().to_path_buf();
        let result = match operation {
            PatchOperation::Replace(request) => self.apply_replace(request),
            PatchOperation::MultiReplace(request) => self.apply_multi_replace(request),
            PatchOperation::WriteFile(request) => self.apply_write_file(request),
        };
        result.map_err(|error| PatchAttemptFailure::from_error(kind, Some(path), error))
    }

    fn apply_preview(&mut self, preview: PatchPreview) -> Result<PatchResult, PatchError> {
        write_text_preserving(
            &preview.path,
            &preview.updated_content,
            preview.encoding,
            preview.line_ending,
        )?;
        let snapshot = self.read_snapshot(&preview.path)?;
        self.reads.insert(preview.path.clone(), snapshot.clone());
        Ok(PatchResult {
            path: preview.path,
            diff: preview.diff,
            snapshot,
            runtime_checkpoint: preview.runtime_checkpoint,
        })
    }

    fn read_snapshot(&self, path: &Path) -> Result<FileSnapshot, PatchError> {
        let metadata = fs::metadata(path).map_err(|e| PatchError::ReadFailed {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        if metadata.len() > self.max_file_bytes {
            return Err(PatchError::FileTooLarge {
                path: path.to_path_buf(),
                bytes: metadata.len(),
                limit: self.max_file_bytes,
            });
        }
        let bytes = fs::read(path).map_err(|e| PatchError::ReadFailed {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        if bytes.contains(&0) {
            return Err(PatchError::BinaryFile {
                path: path.to_path_buf(),
            });
        }
        let encoding = detect_encoding(&bytes);
        let content =
            decode_text(&bytes, encoding).map_err(|message| PatchError::DecodeFailed {
                path: path.to_path_buf(),
                message,
            })?;
        let line_ending = LineEnding::detect(&content);
        let modified = metadata.modified().ok();
        Ok(FileSnapshot {
            path: path.to_path_buf(),
            content,
            modified,
            encoding,
            line_ending,
        })
    }

    fn ensure_not_stale(&self, snapshot: &FileSnapshot) -> Result<(), PatchError> {
        let current = self.read_snapshot(&snapshot.path)?;
        if current.content != snapshot.content {
            return Err(PatchError::StaleRead {
                path: snapshot.path.clone(),
            });
        }
        Ok(())
    }

    fn resolve_workspace_path(&self, path: &Path) -> Result<PathBuf, PatchError> {
        if is_unc_path(path) {
            return Err(PatchError::UnsafePath {
                path: path.to_path_buf(),
                reason: "UNC paths are not allowed for patch operations".into(),
            });
        }
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace_root.join(path)
        };
        let normalized = normalize_path(&joined);
        if !normalized.starts_with(&self.workspace_root) {
            return Err(PatchError::OutsideWorkspace { path: normalized });
        }
        Ok(normalized)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PatchOperation {
    Replace(ReplaceRequest),
    MultiReplace(MultiReplaceRequest),
    WriteFile(WholeFileRequest),
}

impl PatchOperation {
    pub fn kind(&self) -> PatchOperationKind {
        match self {
            PatchOperation::Replace(_) => PatchOperationKind::Replace,
            PatchOperation::MultiReplace(_) => PatchOperationKind::MultiReplace,
            PatchOperation::WriteFile(request) if request.allow_create => {
                PatchOperationKind::CreateFile
            }
            PatchOperation::WriteFile(_) => PatchOperationKind::WholeFileWrite,
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            PatchOperation::Replace(request) => &request.path,
            PatchOperation::MultiReplace(request) => &request.path,
            PatchOperation::WriteFile(request) => &request.path,
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            PatchOperation::Replace(_) => "replace",
            PatchOperation::MultiReplace(_) => "multi_replace",
            PatchOperation::WriteFile(_) => "write_file",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchOperationKind {
    Read,
    Replace,
    MultiReplace,
    WholeFileWrite,
    CreateFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaceRequest {
    pub path: PathBuf,
    pub old_text: String,
    pub new_text: String,
    pub replace_all: bool,
}

impl ReplaceRequest {
    pub fn once(
        path: impl Into<PathBuf>,
        old_text: impl Into<String>,
        new_text: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            old_text: old_text.into(),
            new_text: new_text.into(),
            replace_all: false,
        }
    }

    pub fn all(
        path: impl Into<PathBuf>,
        old_text: impl Into<String>,
        new_text: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            old_text: old_text.into(),
            new_text: new_text.into(),
            replace_all: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiReplaceRequest {
    pub path: PathBuf,
    pub edits: Vec<ReplaceEdit>,
}

impl MultiReplaceRequest {
    pub fn new(path: impl Into<PathBuf>, edits: Vec<ReplaceEdit>) -> Self {
        Self {
            path: path.into(),
            edits,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaceEdit {
    pub old_text: String,
    pub new_text: String,
    pub replace_all: bool,
}

impl ReplaceEdit {
    pub fn once(old_text: impl Into<String>, new_text: impl Into<String>) -> Self {
        Self {
            old_text: old_text.into(),
            new_text: new_text.into(),
            replace_all: false,
        }
    }

    pub fn all(old_text: impl Into<String>, new_text: impl Into<String>) -> Self {
        Self {
            old_text: old_text.into(),
            new_text: new_text.into(),
            replace_all: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WholeFileRequest {
    pub path: PathBuf,
    pub content: String,
    pub allow_create: bool,
    pub encoding: Option<TextEncoding>,
    pub line_ending: Option<LineEnding>,
}

impl WholeFileRequest {
    pub fn replace(path: impl Into<PathBuf>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
            allow_create: false,
            encoding: None,
            line_ending: None,
        }
    }

    pub fn create(path: impl Into<PathBuf>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
            allow_create: true,
            encoding: Some(TextEncoding::Utf8),
            line_ending: None,
        }
    }

    pub fn with_encoding(mut self, encoding: TextEncoding) -> Self {
        self.encoding = Some(encoding);
        self
    }

    pub fn with_line_ending(mut self, line_ending: LineEnding) -> Self {
        self.line_ending = Some(line_ending);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchPreview {
    pub path: PathBuf,
    pub original_content: String,
    pub updated_content: String,
    pub diff: PatchDiff,
    pub encoding: TextEncoding,
    pub line_ending: LineEnding,
    pub runtime_checkpoint: Option<PatchRuntimeCheckpoint>,
}

impl PatchPreview {
    fn new(
        path: PathBuf,
        snapshot: &FileSnapshot,
        updated_content: String,
        replace_all: bool,
    ) -> Self {
        Self {
            path,
            original_content: snapshot.content.clone(),
            diff: PatchDiff::from_contents(&snapshot.content, &updated_content, replace_all),
            updated_content,
            encoding: snapshot.encoding,
            line_ending: snapshot.line_ending,
            runtime_checkpoint: None,
        }
    }

    pub fn with_runtime_checkpoint(mut self, checkpoint: PatchRuntimeCheckpoint) -> Self {
        self.runtime_checkpoint = Some(checkpoint);
        self
    }

    pub fn diff_payload(&self) -> PatchDiffPayload {
        PatchDiffPayload::from_preview(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchResult {
    pub path: PathBuf,
    pub diff: PatchDiff,
    pub snapshot: FileSnapshot,
    pub runtime_checkpoint: Option<PatchRuntimeCheckpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchRuntimeCheckpoint {
    pub deeplossless_snapshot_id: Option<i64>,
    pub deeplossless_rollback_node_id: Option<i64>,
    pub replay_execution_id: Option<String>,
}

impl PatchRuntimeCheckpoint {
    pub fn snapshot(snapshot_id: i64, replay_execution_id: Option<String>) -> Self {
        Self {
            deeplossless_snapshot_id: Some(snapshot_id),
            deeplossless_rollback_node_id: None,
            replay_execution_id,
        }
    }

    pub fn rollback_node(node_id: i64) -> Self {
        Self {
            deeplossless_snapshot_id: None,
            deeplossless_rollback_node_id: Some(node_id),
            replay_execution_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchAttemptFailure {
    pub evidence: PatchFailureEvidence,
    pub error: PatchError,
}

impl PatchAttemptFailure {
    fn from_error(operation: PatchOperationKind, path: Option<PathBuf>, error: PatchError) -> Self {
        Self {
            evidence: PatchFailureEvidence::from_error(operation, path, &error),
            error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchFailureEvidence {
    pub operation: PatchOperationKind,
    pub path: Option<PathBuf>,
    pub error_code: String,
    pub message: String,
    pub recoverable: bool,
    pub suggested_action: String,
}

impl PatchFailureEvidence {
    pub fn from_error(
        operation: PatchOperationKind,
        path: Option<PathBuf>,
        error: &PatchError,
    ) -> Self {
        Self {
            operation,
            path,
            error_code: error.code().to_string(),
            message: error.to_string(),
            recoverable: error.is_recoverable(),
            suggested_action: error.suggested_action().to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchDiff {
    pub changed: bool,
    pub replace_all: bool,
    pub removed_lines: usize,
    pub added_lines: usize,
    pub before_hash: String,
    pub after_hash: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchDiffPayload {
    pub summary: String,
    pub unified_diff: String,
    pub changed: bool,
    pub replace_all: bool,
    pub removed_lines: usize,
    pub added_lines: usize,
    pub before_hash: String,
    pub after_hash: String,
}

impl PatchDiffPayload {
    pub fn from_preview(preview: &PatchPreview) -> Self {
        Self {
            summary: format!(
                "{} removed, {} added",
                preview.diff.removed_lines, preview.diff.added_lines
            ),
            unified_diff: unified_diff(
                &preview.path,
                &preview.original_content,
                &preview.updated_content,
            ),
            changed: preview.diff.changed,
            replace_all: preview.diff.replace_all,
            removed_lines: preview.diff.removed_lines,
            added_lines: preview.diff.added_lines,
            before_hash: preview.diff.before_hash.clone(),
            after_hash: preview.diff.after_hash.clone(),
        }
    }

    pub fn from_diff(diff: &PatchDiff, summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            unified_diff: String::new(),
            changed: diff.changed,
            replace_all: diff.replace_all,
            removed_lines: diff.removed_lines,
            added_lines: diff.added_lines,
            before_hash: diff.before_hash.clone(),
            after_hash: diff.after_hash.clone(),
        }
    }

    pub fn from_summary(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            ..Self::default()
        }
    }
}

impl PatchDiff {
    fn from_contents(before: &str, after: &str, replace_all: bool) -> Self {
        Self {
            changed: before != after,
            replace_all,
            removed_lines: before.lines().count(),
            added_lines: after.lines().count(),
            before_hash: stable_hash(before),
            after_hash: stable_hash(after),
        }
    }
}

fn unified_diff(path: &Path, before: &str, after: &str) -> String {
    if before == after {
        return String::new();
    }

    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();
    let mut prefix = 0;
    while prefix < before_lines.len()
        && prefix < after_lines.len()
        && before_lines[prefix] == after_lines[prefix]
    {
        prefix += 1;
    }

    let mut suffix = 0;
    while suffix + prefix < before_lines.len()
        && suffix + prefix < after_lines.len()
        && before_lines[before_lines.len() - 1 - suffix]
            == after_lines[after_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let context_before_start = prefix.saturating_sub(3);
    let before_changed_end = before_lines.len().saturating_sub(suffix);
    let after_changed_end = after_lines.len().saturating_sub(suffix);
    let context_after_before_end = (before_changed_end + 3).min(before_lines.len());
    let context_after_after_end = (after_changed_end + 3).min(after_lines.len());

    let old_count = context_after_before_end.saturating_sub(context_before_start);
    let new_count = context_after_after_end.saturating_sub(context_before_start);
    let path = path.to_string_lossy().replace('\\', "/");
    let mut output = String::new();
    output.push_str(&format!("--- a/{path}\n"));
    output.push_str(&format!("+++ b/{path}\n"));
    output.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        context_before_start + 1,
        old_count,
        context_before_start + 1,
        new_count
    ));

    for line in &before_lines[context_before_start..prefix] {
        output.push(' ');
        output.push_str(line);
        output.push('\n');
    }
    for line in &before_lines[prefix..before_changed_end] {
        output.push('-');
        output.push_str(line);
        output.push('\n');
    }
    for line in &after_lines[prefix..after_changed_end] {
        output.push('+');
        output.push_str(line);
        output.push('\n');
    }
    for line in &before_lines[before_changed_end..context_after_before_end] {
        output.push(' ');
        output.push_str(line);
        output.push('\n');
    }
    output
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub path: PathBuf,
    pub content: String,
    #[serde(skip)]
    pub modified: Option<SystemTime>,
    pub encoding: TextEncoding,
    pub line_ending: LineEnding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineEnding {
    Lf,
    Crlf,
    Mixed,
    None,
}

impl LineEnding {
    fn detect(content: &str) -> Self {
        let crlf = content.matches("\r\n").count();
        let lf = content.matches('\n').count().saturating_sub(crlf);
        match (crlf, lf) {
            (0, 0) => LineEnding::None,
            (0, _) => LineEnding::Lf,
            (_, 0) => LineEnding::Crlf,
            _ => LineEnding::Mixed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    WorkspaceUnavailable {
        path: PathBuf,
        message: String,
    },
    OutsideWorkspace {
        path: PathBuf,
    },
    UnsafePath {
        path: PathBuf,
        reason: String,
    },
    FileNotRead {
        path: PathBuf,
    },
    FileTooLarge {
        path: PathBuf,
        bytes: u64,
        limit: u64,
    },
    BinaryFile {
        path: PathBuf,
    },
    ReadFailed {
        path: PathBuf,
        message: String,
    },
    WriteFailed {
        path: PathBuf,
        message: String,
    },
    DecodeFailed {
        path: PathBuf,
        message: String,
    },
    ParentDirectoryMissing {
        path: PathBuf,
    },
    StaleRead {
        path: PathBuf,
    },
    EmptyOldText,
    EmptyPatch,
    NoOp,
    TextNotFound,
    AmbiguousMatch {
        matches: usize,
    },
}

impl PatchError {
    pub fn code(&self) -> &'static str {
        match self {
            PatchError::WorkspaceUnavailable { .. } => "workspace_unavailable",
            PatchError::OutsideWorkspace { .. } => "outside_workspace",
            PatchError::UnsafePath { .. } => "unsafe_path",
            PatchError::FileNotRead { .. } => "file_not_read",
            PatchError::FileTooLarge { .. } => "file_too_large",
            PatchError::BinaryFile { .. } => "binary_file",
            PatchError::ReadFailed { .. } => "read_failed",
            PatchError::WriteFailed { .. } => "write_failed",
            PatchError::DecodeFailed { .. } => "decode_failed",
            PatchError::ParentDirectoryMissing { .. } => "parent_directory_missing",
            PatchError::StaleRead { .. } => "stale_read",
            PatchError::EmptyOldText => "empty_old_text",
            PatchError::EmptyPatch => "empty_patch",
            PatchError::NoOp => "no_op",
            PatchError::TextNotFound => "text_not_found",
            PatchError::AmbiguousMatch { .. } => "ambiguous_match",
        }
    }

    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            PatchError::FileNotRead { .. }
                | PatchError::StaleRead { .. }
                | PatchError::TextNotFound
                | PatchError::AmbiguousMatch { .. }
                | PatchError::NoOp
                | PatchError::EmptyPatch
        )
    }

    pub fn suggested_action(&self) -> &'static str {
        match self {
            PatchError::FileNotRead { .. } => "read the target file before patching",
            PatchError::StaleRead { .. } => "re-read the file and regenerate the patch",
            PatchError::TextNotFound => "re-read nearby context and use an exact current match",
            PatchError::AmbiguousMatch { .. } => {
                "use a more specific match or explicitly replace all matches"
            }
            PatchError::NoOp => "drop the no-op edit from the patch plan",
            PatchError::EmptyPatch => "provide at least one edit",
            PatchError::EmptyOldText => "provide non-empty old text",
            PatchError::ParentDirectoryMissing { .. } => {
                "create or choose an existing parent directory"
            }
            PatchError::OutsideWorkspace { .. } | PatchError::UnsafePath { .. } => {
                "choose a path inside the workspace"
            }
            PatchError::BinaryFile { .. }
            | PatchError::FileTooLarge { .. }
            | PatchError::DecodeFailed { .. } => "use a specialized non-text editing path",
            PatchError::WorkspaceUnavailable { .. }
            | PatchError::ReadFailed { .. }
            | PatchError::WriteFailed { .. } => {
                "inspect filesystem state and retry only after resolving the error"
            }
        }
    }
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatchError::WorkspaceUnavailable { path, message } => {
                write!(f, "workspace {} is unavailable: {message}", path.display())
            }
            PatchError::OutsideWorkspace { path } => {
                write!(f, "patch path {} is outside workspace", path.display())
            }
            PatchError::UnsafePath { path, reason } => {
                write!(f, "unsafe patch path {}: {reason}", path.display())
            }
            PatchError::FileNotRead { path } => {
                write!(f, "file {} must be read before patching", path.display())
            }
            PatchError::FileTooLarge { path, bytes, limit } => write!(
                f,
                "file {} is too large for patching: {bytes} > {limit}",
                path.display()
            ),
            PatchError::BinaryFile { path } => {
                write!(f, "file {} appears to be binary", path.display())
            }
            PatchError::ReadFailed { path, message } => {
                write!(f, "failed to read {}: {message}", path.display())
            }
            PatchError::WriteFailed { path, message } => {
                write!(f, "failed to write {}: {message}", path.display())
            }
            PatchError::DecodeFailed { path, message } => {
                write!(f, "failed to decode {}: {message}", path.display())
            }
            PatchError::ParentDirectoryMissing { path } => {
                write!(f, "parent directory {} does not exist", path.display())
            }
            PatchError::StaleRead { path } => {
                write!(f, "file {} changed since last read", path.display())
            }
            PatchError::EmptyOldText => write!(f, "old text cannot be empty"),
            PatchError::EmptyPatch => write!(f, "patch must contain at least one edit"),
            PatchError::NoOp => write!(f, "patch would not change file"),
            PatchError::TextNotFound => write!(f, "old text not found"),
            PatchError::AmbiguousMatch { matches } => {
                write!(f, "old text matched {matches} times; use replace_all")
            }
        }
    }
}

impl std::error::Error for PatchError {}

fn apply_replace(content: &str, request: &ReplaceRequest) -> Result<String, PatchError> {
    if request.old_text.is_empty() {
        return Err(PatchError::EmptyOldText);
    }
    if request.old_text == request.new_text {
        return Err(PatchError::NoOp);
    }
    let matches = content.matches(&request.old_text).count();
    if matches == 0 {
        return Err(PatchError::TextNotFound);
    }
    if matches > 1 && !request.replace_all {
        return Err(PatchError::AmbiguousMatch { matches });
    }
    let updated = if request.replace_all {
        content.replace(&request.old_text, &request.new_text)
    } else {
        content.replacen(&request.old_text, &request.new_text, 1)
    };
    if updated == content {
        return Err(PatchError::NoOp);
    }
    Ok(updated)
}

fn apply_multi_replace(content: &str, edits: &[ReplaceEdit]) -> Result<String, PatchError> {
    if edits.is_empty() {
        return Err(PatchError::EmptyPatch);
    }
    let mut updated = content.to_string();
    for edit in edits {
        updated = apply_replace(
            &updated,
            &ReplaceRequest {
                path: PathBuf::new(),
                old_text: edit.old_text.clone(),
                new_text: edit.new_text.clone(),
                replace_all: edit.replace_all,
            },
        )?;
    }
    if updated == content {
        return Err(PatchError::NoOp);
    }
    Ok(updated)
}

fn detect_encoding(bytes: &[u8]) -> TextEncoding {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        TextEncoding::Utf8Bom
    } else if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        TextEncoding::Utf16Le
    } else {
        TextEncoding::Utf8
    }
}

fn decode_text(bytes: &[u8], encoding: TextEncoding) -> Result<String, String> {
    match encoding {
        TextEncoding::Utf8 => std::str::from_utf8(bytes)
            .map(|s| s.to_string())
            .map_err(|e| e.to_string()),
        TextEncoding::Utf8Bom => std::str::from_utf8(&bytes[3..])
            .map(|s| s.to_string())
            .map_err(|e| e.to_string()),
        TextEncoding::Utf16Le => {
            let data = if bytes.starts_with(&[0xFF, 0xFE]) {
                &bytes[2..]
            } else {
                bytes
            };
            if data.len() % 2 != 0 {
                return Err("odd number of bytes in UTF-16LE content".into());
            }
            let units: Vec<u16> = data
                .chunks_exact(2)
                .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
                .collect();
            String::from_utf16(&units).map_err(|e| e.to_string())
        }
    }
}

fn write_text_preserving(
    path: &Path,
    content: &str,
    encoding: TextEncoding,
    line_ending: LineEnding,
) -> Result<(), PatchError> {
    let content = match line_ending {
        LineEnding::Crlf => content.replace("\r\n", "\n").replace('\n', "\r\n"),
        _ => content.to_string(),
    };
    let bytes = match encoding {
        TextEncoding::Utf8 => content.into_bytes(),
        TextEncoding::Utf8Bom => {
            let mut bytes = vec![0xEF, 0xBB, 0xBF];
            bytes.extend(content.into_bytes());
            bytes
        }
        TextEncoding::Utf16Le => {
            let mut bytes = vec![0xFF, 0xFE];
            for unit in content.encode_utf16() {
                bytes.extend(unit.to_le_bytes());
            }
            bytes
        }
    };
    fs::write(path, bytes).map_err(|e| PatchError::WriteFailed {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn is_unc_path(path: &Path) -> bool {
    path.to_string_lossy().starts_with("\\\\")
}

fn stable_hash(text: &str) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_write_before_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "hello").unwrap();
        let engine = PatchEngine::new(dir.path()).unwrap();

        let err = engine
            .preview_replace(ReplaceRequest::once("a.txt", "hello", "hi"))
            .unwrap_err();

        assert!(matches!(err, PatchError::FileNotRead { .. }));
    }

    #[test]
    fn applies_replace_and_preserves_crlf() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "hello\r\nworld\r\n").unwrap();
        let mut engine = PatchEngine::new(dir.path()).unwrap();
        let snapshot = engine.read("a.txt").unwrap();
        assert_eq!(snapshot.line_ending, LineEnding::Crlf);

        let result = engine
            .apply_replace(ReplaceRequest::once("a.txt", "world", "rust"))
            .unwrap();

        assert!(result.diff.changed);
        let raw = fs::read_to_string(&file).unwrap();
        assert_eq!(raw, "hello\r\nrust\r\n");
    }

    #[test]
    fn rejects_stale_read_when_content_changed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "hello").unwrap();
        let mut engine = PatchEngine::new(dir.path()).unwrap();
        engine.read("a.txt").unwrap();
        fs::write(&file, "changed").unwrap();

        let err = engine
            .preview_replace(ReplaceRequest::once("a.txt", "hello", "hi"))
            .unwrap_err();

        assert!(matches!(
            err,
            PatchError::StaleRead { .. } | PatchError::TextNotFound
        ));
    }

    #[test]
    fn rejects_ambiguous_single_replace() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "one one").unwrap();
        let mut engine = PatchEngine::new(dir.path()).unwrap();
        engine.read("a.txt").unwrap();

        let err = engine
            .preview_replace(ReplaceRequest::once("a.txt", "one", "two"))
            .unwrap_err();

        assert_eq!(err, PatchError::AmbiguousMatch { matches: 2 });
    }

    #[test]
    fn rejects_paths_outside_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = PatchEngine::new(dir.path()).unwrap();

        let err = engine.read("../outside.txt").unwrap_err();

        assert!(matches!(err, PatchError::OutsideWorkspace { .. }));
    }

    #[test]
    fn preview_can_carry_runtime_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "hello").unwrap();
        let mut engine = PatchEngine::new(dir.path()).unwrap();
        engine.read("a.txt").unwrap();

        let preview = engine
            .preview_replace(ReplaceRequest::once("a.txt", "hello", "hi"))
            .unwrap()
            .with_runtime_checkpoint(PatchRuntimeCheckpoint::snapshot(7, Some("exec-1".into())));

        assert_eq!(
            preview
                .runtime_checkpoint
                .as_ref()
                .and_then(|checkpoint| checkpoint.deeplossless_snapshot_id),
            Some(7)
        );
    }

    #[test]
    fn preview_diff_payload_contains_unified_diff_and_stats() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "one\ntwo\nthree\n").unwrap();
        let mut engine = PatchEngine::new(dir.path()).unwrap();
        engine.read("a.txt").unwrap();

        let preview = engine
            .preview_replace(ReplaceRequest::once("a.txt", "two", "2"))
            .unwrap();
        let payload = preview.diff_payload();

        assert!(payload.changed);
        assert_eq!(payload.removed_lines, 3);
        assert_eq!(payload.added_lines, 3);
        assert!(payload.unified_diff.contains("--- a/"));
        assert!(payload.unified_diff.contains("-two"));
        assert!(payload.unified_diff.contains("+2"));
    }

    #[test]
    fn applies_multi_replace_in_one_previewed_write() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();
        let mut engine = PatchEngine::new(dir.path()).unwrap();
        engine.read("a.txt").unwrap();

        let result = engine
            .apply_multi_replace(MultiReplaceRequest::new(
                "a.txt",
                vec![
                    ReplaceEdit::once("alpha", "one"),
                    ReplaceEdit::once("gamma", "three"),
                ],
            ))
            .unwrap();

        assert!(result.diff.changed);
        assert_eq!(fs::read_to_string(&file).unwrap(), "one\nbeta\nthree\n");
    }

    #[test]
    fn failed_multi_replace_does_not_write_partial_changes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "alpha\nbeta\n").unwrap();
        let mut engine = PatchEngine::new(dir.path()).unwrap();
        engine.read("a.txt").unwrap();

        let err = engine
            .apply_multi_replace(MultiReplaceRequest::new(
                "a.txt",
                vec![
                    ReplaceEdit::once("alpha", "one"),
                    ReplaceEdit::once("missing", "two"),
                ],
            ))
            .unwrap_err();

        assert_eq!(err, PatchError::TextNotFound);
        assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\n");
    }

    #[test]
    fn whole_file_write_requires_read_for_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "old").unwrap();
        let mut engine = PatchEngine::new(dir.path()).unwrap();

        let err = engine
            .apply_write_file(WholeFileRequest::replace("a.txt", "new"))
            .unwrap_err();

        assert!(matches!(err, PatchError::FileNotRead { .. }));
        assert_eq!(fs::read_to_string(&file).unwrap(), "old");
    }

    #[test]
    fn whole_file_write_preserves_existing_encoding_and_line_endings() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "\u{feff}old\r\ntext\r\n").unwrap();
        let mut engine = PatchEngine::new(dir.path()).unwrap();
        let snapshot = engine.read("a.txt").unwrap();
        assert_eq!(snapshot.encoding, TextEncoding::Utf8Bom);
        assert_eq!(snapshot.line_ending, LineEnding::Crlf);

        engine
            .apply_write_file(WholeFileRequest::replace("a.txt", "new\ntext\n"))
            .unwrap();

        let raw = fs::read(&file).unwrap();
        assert!(raw.starts_with(&[0xEF, 0xBB, 0xBF]));
        assert_eq!(
            String::from_utf8(raw[3..].to_vec()).unwrap(),
            "new\r\ntext\r\n"
        );
    }

    #[test]
    fn creates_new_file_only_when_explicitly_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("new.txt");
        let mut engine = PatchEngine::new(dir.path()).unwrap();

        let err = engine
            .apply_write_file(WholeFileRequest::replace("new.txt", "hello"))
            .unwrap_err();
        assert!(matches!(err, PatchError::FileNotRead { .. }));

        let result = engine
            .apply_write_file(WholeFileRequest::create("new.txt", "hello\n"))
            .unwrap();

        assert!(result.diff.changed);
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello\n");
    }

    #[test]
    fn create_rejects_missing_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = PatchEngine::new(dir.path()).unwrap();

        let err = engine
            .apply_write_file(WholeFileRequest::create("missing/a.txt", "hello"))
            .unwrap_err();

        assert!(matches!(err, PatchError::ParentDirectoryMissing { .. }));
    }

    #[test]
    fn preview_operation_returns_recovery_evidence_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "hello").unwrap();
        let engine = PatchEngine::new(dir.path()).unwrap();

        let failure = engine
            .preview_operation(PatchOperation::Replace(ReplaceRequest::once(
                "a.txt", "hello", "hi",
            )))
            .unwrap_err();

        assert_eq!(failure.evidence.operation, PatchOperationKind::Replace);
        assert_eq!(failure.evidence.error_code, "file_not_read");
        assert!(failure.evidence.recoverable);
        assert!(failure
            .evidence
            .suggested_action
            .contains("read the target file"));
    }
}
