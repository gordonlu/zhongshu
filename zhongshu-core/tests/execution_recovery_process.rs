use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use zhongshu_core::agent::{
    ExecutionArtifact, ExecutionEffectExpectation, ExecutionEffectIntent, ExecutionGraph,
    ExecutionNode, ExecutionNodeKind, ExecutionNodeState, NodeExecutionOutcome,
};
use zhongshu_core::core::Database;
use zhongshu_core::core::{
    DurableExecutionRunner, ExecutionGraphStore, ExternalFactAssessment, FileClaimFactAdapter,
    FileClaimFactSource, OrganizationCheckpointStore, WorkspaceEffectFactAdapter,
};
use zhongshu_core::integration::DeeplosslessFileClaimFact;
use zhongshu_core::patch::content_hash;

const CHILD_FLAG: &str = "ZHONGSHU_RECOVERY_CRASH_CHILD";
const WINDOW_ENV: &str = "ZHONGSHU_RECOVERY_CRASH_WINDOW";
const DATABASE_ENV: &str = "ZHONGSHU_RECOVERY_DATABASE";
const WORKSPACE_ENV: &str = "ZHONGSHU_RECOVERY_WORKSPACE";
const MARKER_ENV: &str = "ZHONGSHU_RECOVERY_MARKER";
const CLAIM_DATABASE_ENV: &str = "ZHONGSHU_RECOVERY_CLAIM_DATABASE";

fn apply_graph() -> ExecutionGraph {
    let mut graph = ExecutionGraph::new("process-crash-apply").unwrap();
    graph
        .add_node(ExecutionNode::pending(
            "apply",
            ExecutionNodeKind::Apply,
            "apply copy change",
        ))
        .unwrap();
    graph
}

fn apply_intent() -> ExecutionEffectIntent {
    ExecutionEffectIntent {
        id: "apply:workspace:000".into(),
        node_id: "apply".into(),
        expectation: ExecutionEffectExpectation::WorkspaceFile {
            path: "copy.txt".into(),
            before_hash: content_hash("before\n"),
            after_hash: content_hash("after\n"),
            existed_before: true,
        },
    }
}

fn stop_at_window(marker: &Path) -> ! {
    std::fs::write(marker, b"ready").unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[tokio::test]
async fn process_crash_child() {
    if std::env::var_os(CHILD_FLAG).is_none() {
        return;
    }
    let window = std::env::var(WINDOW_ENV).unwrap();
    let database_path = PathBuf::from(std::env::var_os(DATABASE_ENV).unwrap());
    let workspace = PathBuf::from(std::env::var_os(WORKSPACE_ENV).unwrap());
    let marker = PathBuf::from(std::env::var_os(MARKER_ENV).unwrap());
    let claim_database = PathBuf::from(std::env::var_os(CLAIM_DATABASE_ENV).unwrap());
    let database = Database::new(database_path);
    database.migrate().unwrap();
    let store = OrganizationCheckpointStore::new(database);
    let runner = DurableExecutionRunner::new(store);
    if window.starts_with("release_") {
        let mut graph = ExecutionGraph::new("process-crash-release").unwrap();
        graph
            .add_node(ExecutionNode::pending(
                "release",
                ExecutionNodeKind::Release,
                "release claim",
            ))
            .unwrap();
        graph
            .record_effect_intents(
                "release",
                vec![ExecutionEffectIntent {
                    id: "release:claim:000".into(),
                    node_id: "release".into(),
                    expectation: ExecutionEffectExpectation::FileClaim {
                        agent_id: "worker".into(),
                        file_path: "copy.txt".into(),
                        operation: "edit".into(),
                        conv_id: 9,
                        expected_active: false,
                    },
                }],
            )
            .unwrap();
        let mut version = runner.initialize(&graph).await.unwrap();
        runner
            .admit_node(&mut graph, &mut version, "release")
            .await
            .unwrap();
        if window == "release_admitted_before_effect" {
            stop_at_window(&marker);
        }
        let connection = rusqlite::Connection::open(claim_database).unwrap();
        connection
            .execute(
                "DELETE FROM agent_active_files WHERE agent_id = 'worker' AND file_path = 'copy.txt'",
                [],
            )
            .unwrap();
        drop(connection);
        if window == "release_effect_before_outcome" {
            stop_at_window(&marker);
        }
        panic!("unknown release crash window '{window}'");
    }
    let mut graph = apply_graph();
    let mut version = runner.initialize(&graph).await.unwrap();
    if window == "initialized_before_intent" {
        stop_at_window(&marker);
    }

    runner
        .commit_deterministic(&mut graph, &mut version, |candidate| {
            candidate.record_effect_intents("apply", vec![apply_intent()])?;
            Ok(())
        })
        .await
        .unwrap();
    if window == "intent_before_admission" {
        stop_at_window(&marker);
    }

    runner
        .admit_node(&mut graph, &mut version, "apply")
        .await
        .unwrap();
    if window == "admitted_before_effect" {
        stop_at_window(&marker);
    }

    std::fs::write(workspace.join("copy.txt"), "after\n").unwrap();
    if window == "effect_before_outcome" {
        stop_at_window(&marker);
    }

    runner
        .record_outcome(
            &mut graph,
            &mut version,
            "apply",
            &NodeExecutionOutcome::Succeeded(vec![ExecutionArtifact {
                id: "artifact-apply".into(),
                producer_node: "apply".into(),
                kind: "patch_apply".into(),
                summary: "copy changed".into(),
                evidence_refs: vec!["file:copy.txt".into()],
                uncertainties: Vec::new(),
            }]),
        )
        .await
        .unwrap();
    if window == "outcome_persisted" {
        stop_at_window(&marker);
    }
    panic!("unknown crash window '{window}'");
}

fn spawn_crash_child(
    window: &str,
    database: &Path,
    workspace: &Path,
    marker: &Path,
    claim_database: &Path,
) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("process_crash_child")
        .arg("--nocapture")
        .env(CHILD_FLAG, "1")
        .env(WINDOW_ENV, window)
        .env(DATABASE_ENV, database)
        .env(WORKSPACE_ENV, workspace)
        .env(MARKER_ENV, marker)
        .env(CLAIM_DATABASE_ENV, claim_database)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn initialize_claim_database(path: &Path) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE agent_active_files (
                agent_id TEXT NOT NULL,
                file_path TEXT NOT NULL,
                operation TEXT NOT NULL,
                conv_id INTEGER NOT NULL
            );
            INSERT INTO agent_active_files VALUES ('worker', 'copy.txt', 'edit', 9);",
        )
        .unwrap();
}

fn kill_after_marker(mut child: Child, marker: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() && Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("crash child exited before marker with {status}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists(), "crash child did not reach target window");
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success(), "killed child must not exit successfully");
}

#[tokio::test]
async fn kill_restart_classifies_all_apply_persistence_windows() {
    for window in [
        "initialized_before_intent",
        "intent_before_admission",
        "admitted_before_effect",
        "effect_before_outcome",
        "outcome_persisted",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::write(workspace.join("copy.txt"), "before\n").unwrap();
        let database_path = directory.path().join("recovery.db");
        let marker = directory.path().join("marker");
        let claim_database = directory.path().join("claims.db");
        initialize_claim_database(&claim_database);
        let child = spawn_crash_child(window, &database_path, &workspace, &marker, &claim_database);
        kill_after_marker(child, &marker);

        let database = Database::new(database_path);
        database.migrate().unwrap();
        let store = OrganizationCheckpointStore::new(database);
        let runner = DurableExecutionRunner::new(store.clone());
        let recovery = runner
            .recover("process-crash-apply")
            .await
            .unwrap()
            .unwrap();
        let node = recovery.graph.node("apply").unwrap();
        match window {
            "initialized_before_intent" => {
                assert_eq!(node.state, ExecutionNodeState::Pending);
                assert!(recovery.graph.effect_intents_for("apply").is_empty());
                assert_eq!(
                    std::fs::read_to_string(workspace.join("copy.txt")).unwrap(),
                    "before\n"
                );
            }
            "intent_before_admission" => {
                assert_eq!(node.state, ExecutionNodeState::Pending);
                assert_eq!(recovery.graph.effect_intents_for("apply").len(), 1);
                assert_eq!(
                    std::fs::read_to_string(workspace.join("copy.txt")).unwrap(),
                    "before\n"
                );
            }
            "admitted_before_effect" => {
                assert_eq!(node.state, ExecutionNodeState::RecoveryRequired);
                let evidence = WorkspaceEffectFactAdapter::new(&workspace)
                    .assess(&recovery.graph, "apply")
                    .unwrap();
                assert_eq!(evidence.assessment, ExternalFactAssessment::ConfirmedFailed);
            }
            "effect_before_outcome" => {
                assert_eq!(node.state, ExecutionNodeState::RecoveryRequired);
                let evidence = WorkspaceEffectFactAdapter::new(&workspace)
                    .assess(&recovery.graph, "apply")
                    .unwrap();
                assert_eq!(
                    evidence.assessment,
                    ExternalFactAssessment::ConfirmedSucceeded
                );
            }
            "outcome_persisted" => {
                assert_eq!(node.state, ExecutionNodeState::Succeeded);
                assert!(recovery.report.recovery_required_nodes.is_empty());
                assert!(store.list_unfinished_graphs().unwrap().is_empty());
            }
            _ => unreachable!(),
        }
    }
}

struct SqliteClaimFacts {
    path: PathBuf,
}

#[async_trait::async_trait]
impl FileClaimFactSource for SqliteClaimFacts {
    async fn file_claim_facts(&self) -> anyhow::Result<Vec<DeeplosslessFileClaimFact>> {
        let connection = rusqlite::Connection::open(&self.path)?;
        let mut statement = connection
            .prepare("SELECT agent_id, file_path, operation, conv_id FROM agent_active_files")?;
        let rows = statement.query_map([], |row| {
            Ok(DeeplosslessFileClaimFact {
                agent_id: row.get(0)?,
                file_path: row.get(1)?,
                operation: row.get(2)?,
                conv_id: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[tokio::test]
async fn kill_restart_classifies_release_before_and_after_external_effect() {
    for (window, expected) in [
        (
            "release_admitted_before_effect",
            ExternalFactAssessment::ConfirmedFailed,
        ),
        (
            "release_effect_before_outcome",
            ExternalFactAssessment::ConfirmedSucceeded,
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let database_path = directory.path().join("recovery.db");
        let claim_database = directory.path().join("claims.db");
        initialize_claim_database(&claim_database);
        let marker = directory.path().join("marker");
        let child = spawn_crash_child(window, &database_path, &workspace, &marker, &claim_database);
        kill_after_marker(child, &marker);

        let database = Database::new(database_path);
        database.migrate().unwrap();
        let store = OrganizationCheckpointStore::new(database);
        let recovery = DurableExecutionRunner::new(store)
            .recover("process-crash-release")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            recovery.graph.node("release").unwrap().state,
            ExecutionNodeState::RecoveryRequired
        );
        let source = SqliteClaimFacts {
            path: claim_database,
        };
        let evidence = FileClaimFactAdapter::new(&source)
            .assess(&recovery.graph, "release")
            .await
            .unwrap();
        assert_eq!(evidence.assessment, expected);
    }
}
