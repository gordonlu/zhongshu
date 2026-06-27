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

    pub fn apply_replace(&mut self, request: ReplaceRequest) -> Result<PatchResult, PatchError> {
        let preview = self.preview_replace(request)?;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchDiff {
    pub changed: bool,
    pub replace_all: bool,
    pub removed_lines: usize,
    pub added_lines: usize,
    pub before_hash: String,
    pub after_hash: String,
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
    StaleRead {
        path: PathBuf,
    },
    EmptyOldText,
    NoOp,
    TextNotFound,
    AmbiguousMatch {
        matches: usize,
    },
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
            PatchError::StaleRead { path } => {
                write!(f, "file {} changed since last read", path.display())
            }
            PatchError::EmptyOldText => write!(f, "old text cannot be empty"),
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
}
