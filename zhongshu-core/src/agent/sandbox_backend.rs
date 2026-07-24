use std::fmt::Debug;
use std::path::PathBuf;

use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct SandboxRequest {
    pub command: String,
    pub workdir: PathBuf,
    pub envs: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct SandboxOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone)]
pub enum SandboxError {
    SpawnFailed(String),
    BinaryMissing,
    PermissionDenied(String),
}

#[async_trait]
pub trait SandboxBackend: Debug + Send + Sync {
    async fn execute(&self, request: SandboxRequest) -> Result<SandboxOutput, SandboxError>;
    fn box_clone(&self) -> Box<dyn SandboxBackend>;
}

#[derive(Debug)]
pub struct BubblewrapSandboxBackend;

impl BubblewrapSandboxBackend {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SandboxBackend for BubblewrapSandboxBackend {
    async fn execute(&self, request: SandboxRequest) -> Result<SandboxOutput, SandboxError> {
        if !PathBuf::from("/usr/bin/bwrap").exists() {
            return Err(SandboxError::BinaryMissing);
        }

        let mut process = tokio::process::Command::new("/usr/bin/bwrap");
        process.kill_on_drop(true);

        process.args([
            "--die-with-parent",
            "--new-session",
            "--unshare-all",
            "--share-net",
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

        if let Ok(resolved) = std::fs::canonicalize("/bin") {
            process.args(["--ro-bind", resolved.to_str().unwrap(), "/bin"]);
        }
        process.args(["--ro-bind", "/etc", "/etc"]);
        process.arg("--bind");
        process.arg(&request.workdir).arg("/workspace");

        for path in ["/lib", "/lib64"] {
            if let Ok(resolved) = std::fs::canonicalize(path) {
                process.args(["--ro-bind", resolved.to_str().unwrap(), path]);
            }
        }

        for (variable, fallback) in [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")] {
            let path = std::env::var_os(variable).map(PathBuf::from).or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(fallback))
            });
            if let Some(path) = path.filter(|p| p.exists()) {
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
            &request.command,
        ]);

        let output = process
            .output()
            .await
            .map_err(|e| SandboxError::SpawnFailed(e.to_string()))?;

        let exit_code = output.status.code().unwrap_or(-1);
        Ok(SandboxOutput {
            exit_code,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    fn box_clone(&self) -> Box<dyn SandboxBackend> {
        Box::new(Self)
    }
}

impl Default for BubblewrapSandboxBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct TestSandboxBackend;

impl TestSandboxBackend {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SandboxBackend for TestSandboxBackend {
    async fn execute(&self, request: SandboxRequest) -> Result<SandboxOutput, SandboxError> {
        let output = tokio::process::Command::new("/usr/bin/sh")
            .arg("-lc")
            .arg(&request.command)
            .current_dir(&request.workdir)
            .envs(request.envs)
            .output()
            .await
            .map_err(|e| SandboxError::SpawnFailed(e.to_string()))?;

        Ok(SandboxOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    fn box_clone(&self) -> Box<dyn SandboxBackend> {
        Box::new(Self)
    }
}

impl Default for TestSandboxBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SandboxAvailability {
    Available,
    Unavailable { reason: String },
}

pub async fn probe_sandbox_availability() -> SandboxAvailability {
    match tokio::process::Command::new("/usr/bin/bwrap")
        .args([
            "--unshare-user",
            "--uid",
            "0",
            "--gid",
            "0",
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "true",
        ])
        .output()
        .await
    {
        Ok(output) if output.status.success() => SandboxAvailability::Available,
        Ok(output) => SandboxAvailability::Unavailable {
            reason: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Err(e) => SandboxAvailability::Unavailable {
            reason: format!("failed to spawn bwrap: {e}"),
        },
    }
}
