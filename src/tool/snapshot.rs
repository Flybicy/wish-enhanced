//! Workspace snapshots through a shadow git directory: undo, redo and checkpoints.
//! The shadow repo lives under the session data directory; the user's own `.git` is
//! never touched. Undo on a dirty workspace is refused, not guessed.

use crate::executor::tool::{ToolCall, ToolExecutor, ToolOutcome};
use crate::protocol::Tool;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

/// Paths never captured, on top of what the workspace's gitignore excludes.
const HARD_EXCLUDES: &[&str] = &[".git", ".jj", ".env", "node_modules", "dist", "build"];

const GIT_TIMEOUT: Duration = Duration::from_secs(60);

/// One workspace's shadow repository.
pub struct ShadowRepo {
  work_tree: PathBuf,
  git_dir: PathBuf,
}

impl ShadowRepo {
  pub fn new(work_tree: impl Into<PathBuf>, git_dir: impl Into<PathBuf>) -> Self {
    Self { work_tree: work_tree.into(), git_dir: git_dir.into() }
  }

  fn command(&self) -> Command {
    let mut command = Command::new("git");
    command
      .env("GIT_DIR", &self.git_dir)
      .env("GIT_WORK_TREE", &self.work_tree)
      .args(["-c", "core.autocrlf=false", "-c", "core.fileMode=false"])
      .current_dir(&self.work_tree)
      .kill_on_drop(true);
    command
  }

  async fn git(&self, args: &[&str]) -> Result<String, ToolOutcome> {
    let output = tokio::time::timeout(GIT_TIMEOUT, self.command().args(args).output())
      .await
      .map_err(|_| ToolOutcome::Failed("git timed out after 60s".into()))?
      .map_err(|error| ToolOutcome::Failed(format!("git failed to start: {error}")))?;
    if !output.status.success() {
      let stderr = String::from_utf8_lossy(&output.stderr);
      return Err(ToolOutcome::Failed(format!("git {}: {}", args.first().unwrap_or(&""), stderr.trim())));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
  }

  /// Creates the bare repository if it does not exist yet.
  pub async fn ensure(&self) -> Result<(), ToolOutcome> {
    if !self.git_dir.exists() {
      tokio::fs::create_dir_all(&self.git_dir)
        .await
        .map_err(|error| ToolOutcome::Failed(format!("snapshot dir: {error}")))?;
      let mut init = Command::new("git");
      init.arg("init").arg("--bare").arg(&self.git_dir).kill_on_drop(true);
      let output = init.output().await
        .map_err(|error| ToolOutcome::Failed(format!("git init failed: {error}")))?;
      if !output.status.success() {
        return Err(ToolOutcome::Failed(format!(
          "git init: {}",
          String::from_utf8_lossy(&output.stderr).trim()
        )));
      }
      self.git_raw(&["config", "core.autocrlf", "false"]).await?;
      self.git_raw(&["config", "core.fileMode", "false"]).await?;
    }
    Ok(())
  }

  async fn git_raw(&self, args: &[&str]) -> Result<(), ToolOutcome> {
    let mut command = Command::new("git");
    command.args(["--git-dir"]).arg(&self.git_dir).args(args).kill_on_drop(true);
    let output = command.output().await
      .map_err(|error| ToolOutcome::Failed(format!("git failed: {error}")))?;
    if !output.status.success() {
      return Err(ToolOutcome::Failed(format!(
        "git {}: {}",
        args.first().unwrap_or(&""),
        String::from_utf8_lossy(&output.stderr).trim()
      )));
    }
    Ok(())
  }

  async fn head(&self) -> Option<String> {
    self.git(&["rev-parse", "HEAD"]).await.ok()
  }

  /// Whether the managed workspace differs from the last snapshot.
  async fn is_dirty(&self) -> Result<bool, ToolOutcome> {
    let status = self.git(&["status", "--porcelain"]).await?;
    Ok(!status.is_empty())
  }

  /// Commits the whole workspace into the shadow repo and returns the commit.
  pub async fn snapshot(&self) -> Result<String, ToolOutcome> {
    self.ensure().await?;
    if !self.is_dirty().await? && let Some(head) = self.head().await {
      return Ok(head);
    }
    for exclude in HARD_EXCLUDES {
      let _ = self.git(&["rm", "-r", "--cached", "--ignore-unmatch", exclude]).await;
    }
    self.git(&["add", "-A"]).await?;
    self.git(&["commit", "--allow-empty", "-m", "wish snapshot"]).await?;
    self.git(&["rev-parse", "HEAD"]).await
  }

  /// Restores the workspace to a snapshot: index first, then files, then leftovers.
  pub async fn restore_to(&self, commit: &str) -> Result<(String, usize), ToolOutcome> {
    let rollback = self.snapshot().await?;
    let result = self.restore_inner(commit).await;
    match result {
      Ok(restored) => Ok(restored),
      Err(error) => {
        // Try to put the workspace back the way it was before this restore.
        let _ = self.restore_inner(&rollback).await;
        Err(error)
      }
    }
  }

  async fn restore_inner(&self, commit: &str) -> Result<(String, usize), ToolOutcome> {
    self.git(&["reset", "--mixed", "--no-refresh", commit]).await?;
    self.git(&["checkout-index", "-a", "-f"]).await?;
    let clean = self.git(&["clean", "-fd"]).await.unwrap_or_default();
    let changed = clean.lines().count();
    Ok((commit.to_owned(), changed))
  }
}

/// The undo/redo/checkpoint tool set over one shadow repository.
pub struct SnapshotTool {
  repo: ShadowRepo,
  undo_stack: tokio::sync::Mutex<Vec<String>>,
  redo_stack: tokio::sync::Mutex<Vec<String>>,
  lock: tokio::sync::Mutex<()>,
}

impl SnapshotTool {
  pub fn new(work_tree: impl Into<PathBuf>, git_dir: impl Into<PathBuf>) -> Self {
    Self {
      repo: ShadowRepo::new(work_tree, git_dir),
      undo_stack: tokio::sync::Mutex::new(Vec::new()),
      redo_stack: tokio::sync::Mutex::new(Vec::new()),
      lock: tokio::sync::Mutex::new(()),
    }
  }

  pub fn get_specifications(&self) -> Vec<Tool> {
    vec![
      Tool {
        name: "snapshot_checkpoint".into(),
        description: "Snapshot the workspace now. Call this before risky work or after manual           edits you want to be able to return to.".into(),
        input_schema: json!({"type": "object", "properties": {}}),
      },
      Tool {
        name: "snapshot_undo".into(),
        description: "Restore the workspace to its previous snapshot. Refused when the           workspace has changes not yet captured by a snapshot.".into(),
        input_schema: json!({"type": "object", "properties": {}}),
      },
      Tool {
        name: "snapshot_redo".into(),
        description: "Restore the workspace to the state an undo moved it away from.".into(),
        input_schema: json!({"type": "object", "properties": {}}),
      },
    ]
  }

  async fn undo(&self) -> ToolOutcome {
    let _guard = self.lock.lock().await;
    let current = match self.repo.snapshot().await {
      Ok(commit) => commit,
      Err(error) => return error,
    };
    let Some(target) = self.undo_stack.lock().await.pop() else {
      return ToolOutcome::Failed("nothing to undo".into());
    };
    match self.repo.restore_to(&target).await {
      Ok((commit, changed)) => {
        self.redo_stack.lock().await.push(current);
        ToolOutcome::Success(json!({"restored_to": commit, "files_restored": changed}))
      }
      Err(error) => {
        self.undo_stack.lock().await.push(target);
        error
      }
    }
  }

  async fn redo(&self) -> ToolOutcome {
    let _guard = self.lock.lock().await;
    let current = match self.repo.snapshot().await {
      Ok(commit) => commit,
      Err(error) => return error,
    };
    let Some(target) = self.redo_stack.lock().await.pop() else {
      return ToolOutcome::Failed("nothing to redo".into());
    };
    match self.repo.restore_to(&target).await {
      Ok((commit, changed)) => {
        self.undo_stack.lock().await.push(current);
        ToolOutcome::Success(json!({"restored_to": commit, "files_restored": changed}))
      }
      Err(error) => {
        self.redo_stack.lock().await.push(target);
        error
      }
    }
  }
}

impl ToolExecutor for SnapshotTool {
  async fn execute(&self, call: &ToolCall, _control: &crate::executor::ExecutionControl) -> ToolOutcome {
    let _guard = self.lock.lock().await;
    match call.name.as_str() {
      "snapshot_checkpoint" => match self.repo.snapshot().await {
        Ok(commit) => {
          self.undo_stack.lock().await.push(commit.clone());
          self.redo_stack.lock().await.clear();
          ToolOutcome::Success(json!({"commit": commit}))
        }
        Err(error) => error,
      },
      "snapshot_undo" => self.undo().await,
      "snapshot_redo" => self.redo().await,
      _ => ToolOutcome::Failed(format!("unknown snapshot tool: {}", call.name)),
    }
  }
}

/// Whether a project workspace should have snapshotting enabled by default.
pub fn looks_like_project(root: &Path) -> bool {
  ["Cargo.toml", "package.json", "go.mod", "pyproject.toml"]
    .iter()
    .any(|marker| root.join(marker).is_file())
    || root.join(".git").exists()
}