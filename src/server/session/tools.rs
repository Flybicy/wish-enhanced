use crate::server::{app::App, error::blocking};
use crate::{
  executor::{
    ExecutionControl,
    tool::{ToolCall, ToolExecutor, ToolOutcome},
  },
  protocol::{ContentBlock, Message},
  session::{PermissionMode, Session, SessionHandle},
  tool::{search_history::SearchHistoryTool, shell::ShellTool, view_image::ViewImageTool},
};
use serde_json::{json, Value};
use std::{
  path::PathBuf,
  sync::{Arc, Weak, atomic::Ordering},
};

pub struct SessionTools {
  pub shell: Option<ShellTool>,
  history: SearchHistoryTool,
  pub(crate) app: Weak<App>,
  session_id: String,
  handle: SessionHandle,
  image_dir: PathBuf,
  /// Workspace snapshots for undo and redo; present when snapshotting is enabled.
  pub snapshot: Option<crate::tool::snapshot::SnapshotTool>,
  /// Prompt skills loaded from the data directory.
  pub skills: Option<crate::server::skills::SkillSearchTool>,
  /// Long-term memory recall over the shared ledger.
  pub memory: Option<crate::session::memory::MemoryRecallTool>,
}
impl SessionTools {
  pub fn new(
    shell: Option<ShellTool>,
    session: &Session,
    app: Weak<App>,
    session_id: String,
    image_dir: PathBuf,
    snapshot: Option<crate::tool::snapshot::SnapshotTool>,
    skills: Option<crate::server::skills::SkillSearchTool>,
    memory: Option<crate::session::memory::MemoryRecallTool>,
  ) -> Self {
    Self {
      shell,
      history: SearchHistoryTool::new(session),
      app,
      session_id,
      handle: session.create_handle(),
      image_dir,
      snapshot,
      skills,
      memory,
    }
  }
  pub fn get_history_specifications(&self) -> Vec<crate::protocol::Tool> {
    self.history.get_specifications()
  }
  /// Whether the application has web tools enabled.
  pub fn web_enabled(&self) -> bool {
    self.app.upgrade().map(|app| app.web.is_some()).unwrap_or(false)
  }
  /// The web tool schemas, when the application has them enabled.
  pub fn web_specifications(&self) -> Option<Vec<crate::protocol::Tool>> {
    self.app.upgrade().and_then(|app| app.web.clone()).map(|web| web.get_specifications())
  }
  /// Whether subagent delegation is enabled in the configuration.
  pub fn subagents_enabled(&self) -> bool {
    self.app.upgrade().map(|app| app.subagents_enabled).unwrap_or(false)
  }
  /// An owning handle to the application, when it is still alive.
  pub fn app_handle(&self) -> Option<Arc<App>> {
    self.app.upgrade()
  }
  /// Whether an MCP server with this ID started successfully.
  pub fn mcp_registered(&self, server: &str) -> bool {
    self
      .app
      .upgrade()
      .map(|app| app.mcp_tools.lock().unwrap().contains_key(server))
      .unwrap_or(false)
  }
  /// The tool schemas of every running MCP server, captured at startup and
  /// namespaced as mcp_<server>_<tool>. Synchronous by design: no MCP I/O here.
  pub fn mcp_specifications(&self) -> Vec<crate::protocol::Tool> {
    let Some(app) = self.app.upgrade() else {
      return Vec::new();
    };
    let specs = app.mcp_tools.lock().unwrap();
    specs.values().flatten().cloned().collect()
  }
  async fn execute_shell(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    let Some(shell) = &self.shell else {
      return ToolOutcome::Failed("shell is not enabled for this session".into());
    };
    let app = self.app.upgrade();
    let generation = match &app {
      Some(app) => app
        .get_session(&self.session_id)
        .await
        .ok()
        .map(|slot| slot.auto_run_generation.load(Ordering::SeqCst)),
      None => None,
    };
    let outcome = shell.execute(call, control).await;
    let ToolOutcome::Success(output) = &outcome else {
      return outcome;
    };
    let is_start = call.name == "shell_start";
    if !is_start || output["process"]["status"] != "running" {
      return outcome;
    }
    let (Some(id), Some(app)) = (output["execution_id"].as_str(), app) else {
      return outcome;
    };
    let (shell, id, handle, session_id, command) = (
      shell.clone(),
      id.to_owned(),
      self.handle.clone(),
      self.session_id.clone(),
      call.arguments["command"].clone(),
    );
    let owner = app.clone();
    app.tasks.spawn(async move {
      let result = tokio::select! {
        _ = owner.stop.cancelled() => return,
        result = shell.wait_for_completion(&id) => result,
      };
      let result = match result {
        Ok(result) => result,
        Err(error) => json!({"execution_id":id,"error":error.to_string()}),
      };
      let message = Message::Developer {
        metadata: json!({"source":"background_execution_finished","execution_id":id,"completion":{"command":command,"result":result}}),
        fixed: Some(false),
        content: vec![ContentBlock::Text {
          text: format!(
            "A background shell execution reached a terminal state.\n{}",
            json!({"command":command,"result":result})
          ),
        }],
      };
      if let Err(error) = blocking(move || Ok(handle.enqueue_message(message)?)).await {
        eprintln!("background notification: {error}");
        return;
      }
      if let Ok(slot) = owner.get_session(&session_id).await {
        let _ = owner.events.send(json!({"type":"session_changed","id":session_id}));
        if generation == Some(slot.auto_run_generation.load(Ordering::SeqCst)) {
          crate::server::http::content::schedule(owner, slot);
        }
      }
    });
    outcome
  }
}
impl ToolExecutor for SessionTools {
  async fn execute(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    let app = self.app.upgrade();
    let slot = match &app {
      Some(app) => app.get_session(&self.session_id).await.ok(),
      None => None,
    };
    let mut approval = None;
    let outcome = if has_side_effects(&call.name) {
      match (app.as_ref(), slot.as_ref()) {
        (None, _) => ToolOutcome::Failed("application is shutting down".into()),
        (_, None) => ToolOutcome::Failed("session is unavailable".into()),
        (Some(app), Some(slot)) => match slot.permission() {
          PermissionMode::Operate => self.dispatch(call, control).await,
          PermissionMode::ReadOnly => denied_read_only(&call.name),
          PermissionMode::Ask => match self.request_approval(app, slot, call, control).await {
            Approval::Granted => {
              approval = Some("granted");
              self.dispatch(call, control).await
            }
            Approval::Denied => {
              approval = Some("denied");
              ToolOutcome::Failed("denied by user".into())
            }
            Approval::Cancelled => {
              approval = Some("cancelled");
              ToolOutcome::Cancelled
            }
            Approval::Error(message) => ToolOutcome::Failed(message),
          },
        },
      }
    } else {
      self.dispatch(call, control).await
    };
    // Minimal audit trail on the session's own event stream (the export path).
    // It records the outcome as a name and a redacted argument summary only —
    // never the tool's payload, which may embed secrets.
    if let Some(slot) = &slot {
      let _ = slot.events.send(json!({
        "type": "tool_audit",
        "tool": call.name,
        "call_id": call.call_id,
        "session_id": self.session_id,
        "generation": slot.auto_run_generation.load(Ordering::SeqCst),
        "approval": approval,
        "outcome": outcome_name(&outcome),
        "time": crate::session::statistics::Timestamp::now().0,
        "args": preview(&call.arguments),
      }));
    }
    outcome
  }
}

impl SessionTools {
  async fn dispatch(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    match call.name.as_str() {
      "view_image" => {
        let outcome = ViewImageTool.execute(call, control).await;
        if let ToolOutcome::SuccessWithInput { mut output, mut input } = outcome {
          for block in &input {
            if let ContentBlock::Image { data_base64, .. } = block {
              let (directory, data) = (self.image_dir.clone(), data_base64.clone());
              match blocking(move || {
                crate::server::media::save_image(&directory, &data)
                  .map_err(crate::server::error::ApiError::internal)
              })
              .await
              {
                Ok(path) => output["session_path"] = json!(path),
                Err(error) => return ToolOutcome::Failed(error.to_string()),
              }
            }
          }
          input.retain(|block| matches!(block, ContentBlock::Image { .. }));
          ToolOutcome::SuccessWithInput { output, input }
        } else {
          outcome
        }
      }
      "history_search" | "history_read" | "history_query" => {
        self.history.execute(call, control).await
      }
      "shell_start" | "shell_edit" | "shell_poll" | "shell_write" | "shell_kill" => {
        self.execute_shell(call, control).await
      }
      "web_search" | "fetch_content" | "get_search_content" => {
        let Some(app) = self.app.upgrade() else {
          return ToolOutcome::Failed("application is shutting down".into());
        };
        let Some(web) = app.web.clone() else {
          return ToolOutcome::Failed("web tools are disabled in the configuration".into());
        };
        web.execute(call, control).await
      }
      "snapshot_checkpoint" | "snapshot_undo" | "snapshot_redo" => {
        let Some(snapshot) = &self.snapshot else {
          return ToolOutcome::Failed("workspace snapshots are disabled for this session".into());
        };
        snapshot.execute(call, control).await
      }
      "skill_search" => {
        let Some(skills) = &self.skills else {
          return ToolOutcome::Failed("skills are disabled in the configuration".into());
        };
        skills.execute(call)
      }
      "memory_recall" => {
        let Some(memory) = &self.memory else {
          return ToolOutcome::Failed("memory is disabled in the configuration".into());
        };
        memory.execute(call, control).await
      }
      "subagent" | "subagent_result" => {
        let Some(app) = self.app.upgrade() else {
          return ToolOutcome::Failed("application is shutting down".into());
        };
        execute_subagent_tool(&app, &self.session_id, call).await
      }
      name if name.starts_with("mcp_") => {
        let Some(app) = self.app.upgrade() else {
          return ToolOutcome::Failed("application is shutting down".into());
        };
        let rest = &name["mcp_".len()..];
        let Some((server, tool)) = rest.split_once('_') else {
          return ToolOutcome::Failed(format!("malformed mcp tool name: {name}"));
        };
        // Remote HTTP servers first; stdio registry second. The guard must be
        // dropped before the await point.
        let http = app.mcp_http.lock().unwrap().get(server).cloned();
        if let Some(http) = http {
          return http.call_tool(tool, call.arguments.clone(), control).await;
        }
        let Some(spec) = app.mcp_specs.lock().await.get(server).cloned() else {
          return ToolOutcome::Failed(format!("unknown mcp server: {server}"));
        };
        app.mcp.call_tool(server, tool, call.arguments.clone(), &spec, control).await
      }
      _ => ToolOutcome::Failed(format!("unknown tool: {}", call.name)),
    }
  }
}

/// The outcome of an ask-mode approval request.
enum Approval {
  Granted,
  Denied,
  Cancelled,
  Error(String),
}
impl From<bool> for Approval {
  fn from(granted: bool) -> Self {
    if granted { Self::Granted } else { Self::Denied }
  }
}

/// How long a pending approval may wait before the executor stops blocking
/// the run. A missed decision aborts the call; it never auto-grants.
const APPROVAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Removes the approval entry when dropped. Held across every await of the
/// request — including cancellation — so a dropped future can never leave a
/// dangling sender behind in the registry.
struct ApprovalGuard<'a> {
  app: &'a Arc<App>,
  slot: &'a Arc<super::SessionSlot>,
  key: crate::server::app::ApprovalKey,
}
impl Drop for ApprovalGuard<'_> {
  fn drop(&mut self) {
    self.app.approvals.lock().unwrap().remove(&self.key);
    self.slot.clear_pending_approval(&self.key.call_id, self.key.generation);
  }
}

impl SessionTools {
  /// Ask mode: publish the call as awaiting approval on the session's event
  /// stream and wait for the user's decision. The entry is keyed by the run
  /// generation that asked and removed by a drop guard on every exit path,
  /// so nothing leaks and a stale answer cannot resolve a newer run.
  async fn request_approval(
    &self,
    app: &Arc<App>,
    slot: &Arc<super::SessionSlot>,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> Approval {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let key = crate::server::app::ApprovalKey {
      session_id: self.session_id.clone(),
      generation: slot.auto_run_generation.load(Ordering::SeqCst),
      call_id: call.call_id.clone(),
    };
    {
      let mut approvals = app.approvals.lock().unwrap();
      if approvals.contains_key(&key) {
        return Approval::Error("duplicate approval request".into());
      }
      approvals.insert(key.clone(), sender);
    }
    let _guard = ApprovalGuard { app, slot, key: key.clone() };
    let summary = preview(&call.arguments);
    let _ = slot.events.send(json!({
      "type": "approval_required",
      "call_id": call.call_id,
      "name": call.name,
      "generation": key.generation,
      "summary": summary.clone(),
    }));
    // Keep the card in the status snapshot AND the live preview so a UI
    // refresh or SSE reconnect cannot lose it.
    slot.note_pending_approval(&call.call_id, &call.name, &summary, key.generation);
    let approval = tokio::select! {
      decision = receiver => match decision {
        Ok(granted) => Approval::from(granted),
        // The sender was dropped (interrupt, shutdown, mode switch): the run
        // moved on without a decision, so the call is cancelled, not granted.
        Err(_) => Approval::Cancelled,
      },
      _ = control.wait_for_cancellation() => Approval::Cancelled,
      _ = tokio::time::sleep(APPROVAL_TIMEOUT) => Approval::Error("approval request timed out".into()),
    };
    approval
  }
}

/// Tools that only observe state this session already owns. The allowlist is
/// positive evidence: anything not named here — including unknown, future or
/// MCP-exposed tools — is treated as a side effect.
pub(crate) fn is_read_only(name: &str) -> bool {
  matches!(
    name,
    "history_search"
      | "history_read"
      | "history_query"
      | "shell_poll"
      | "view_image"
      | "skill_search"
      | "memory_recall"
      | "subagent_result"
  )
}
/// Whether the tool must pass the session's permission gate. Deny by default:
/// a tool name that is not positively read-only can never escape read-only or
/// ask mode by simply being unlisted.
pub(crate) fn has_side_effects(name: &str) -> bool {
  !is_read_only(name)
}

fn denied_read_only(name: &str) -> ToolOutcome {
  ToolOutcome::Failed(format!("denied: session is read-only ({name} is a side-effect tool)"))
}

/// Keys whose values must never reach a UI card, an SSE event or a log line.
fn is_sensitive_key(key: &str) -> bool {
  const MARKERS: [&str; 13] = [
    "token",
    "secret",
    "password",
    "passwd",
    "credential",
    "authoriz",
    "cookie",
    "private_key",
    "api_key",
    "apikey",
    "auth",
    "bearer",
    "passphrase",
  ];
  let key = key.to_lowercase();
  MARKERS.iter().any(|marker| key.contains(marker))
}
/// Masks credentials embedded in free text: `Bearer <credential>` headers and
/// `password=…`-style assignments. Unrelated text passes through untouched.
fn mask_credentials(text: &str) -> String {
  const KEYS: [&str; 6] = ["password", "passwd", "token", "secret", "api_key", "apikey"];
  let lower = text.to_ascii_lowercase();
  let mut out = String::with_capacity(text.len());
  let mut cursor = 0;
  while cursor < text.len() {
    if lower[cursor..].starts_with("bearer") {
      let rest = &text[cursor + "bearer".len()..];
      let spaces = rest.len() - rest.trim_start_matches([' ', '\t']).len();
      let value_start = cursor + "bearer".len() + spaces;
      if spaces > 0 && value_start < text.len() {
        out.push_str(&text[cursor..value_start]);
        out.push_str("[redacted]");
        let value_len = text[value_start..]
          .find(|ch: char| ch.is_whitespace() || ch == '\'' || ch == '"')
          .unwrap_or(text.len() - value_start);
        cursor = value_start + value_len;
        continue;
      }
    }
    if let Some(key) = KEYS.iter().find(|key| lower[cursor..].starts_with(**key)) {
      let assign = cursor + key.len();
      if text[assign..].starts_with('=') {
        let value_start = assign + 1;
        let value_len = text[value_start..]
          .find(|ch: char| ch.is_whitespace() || ch == '\'' || ch == '"')
          .unwrap_or(text.len() - value_start);
        out.push_str(&text[cursor..value_start]);
        out.push_str("[redacted]");
        cursor = value_start + value_len;
        continue;
      }
    }
    let ch = text[cursor..].chars().next().unwrap();
    out.push(ch);
    cursor += ch.len_utf8();
  }
  out
}
/// Redacts everything that leaves the process through approval cards or the
/// audit stream: sensitive keys are replaced recursively, and well-known
/// credential carriers inside strings are masked in place.
pub(crate) fn redact(value: &Value) -> Value {
  match value {
    Value::Object(map) => Value::Object(
      map
        .iter()
        .map(|(key, item)| {
          if is_sensitive_key(key) {
            (key.clone(), Value::String("[redacted]".into()))
          } else {
            (key.clone(), redact(item))
          }
        })
        .collect(),
    ),
    Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
    Value::String(text) => Value::String(mask_credentials(text)),
    other => other.clone(),
  }
}
/// The audit outcome is a name, never the payload: a tool's own output may
/// embed secrets and must not widen the logging surface.
fn outcome_name(outcome: &ToolOutcome) -> &'static str {
  match outcome {
    ToolOutcome::Success(_) => "success",
    ToolOutcome::SuccessWithInput { .. } => "success_with_input",
    ToolOutcome::SuccessWithMetadata { .. } => "success_with_metadata",
    ToolOutcome::Failed(_) => "failed",
    ToolOutcome::Cancelled => "cancelled",
    ToolOutcome::Unknown(_) => "unknown",
  }
}
/// Upper bound of a previewed argument blob. A long shell command needs most
/// of its tail visible to be reviewable, so the budget is generous; the cap
/// still exists so one argument cannot flood the event stream.
const PREVIEW_LIMIT: usize = 2048;

/// A compact JSON preview of a call's arguments for the approval card and the
/// audit event. Redaction runs before truncation so the cut cannot hide an
/// unmasked tail.
fn preview(arguments: &Value) -> String {
  let text = redact(arguments).to_string();
  if text.chars().count() <= PREVIEW_LIMIT {
    return text;
  }
  let mut summary: String = text.chars().take(PREVIEW_LIMIT).collect();
  summary.push('…');
  summary
}

#[cfg(test)]
mod tests {
  use super::{denied_read_only, has_side_effects, is_read_only, mask_credentials, preview, redact};
  use crate::executor::tool::ToolOutcome;
  use serde_json::json;

  #[test]
  fn side_effect_tools_are_classified() {
    for name in [
      "shell_start",
      "shell_edit",
      "shell_write",
      "shell_kill",
      "snapshot_undo",
      "snapshot_redo",
      "snapshot_checkpoint",
      "subagent",
      "web_search",
      "fetch_content",
      "get_search_content",
      "mcp_server_tool",
      // Unknown or future names are side effects until positively read-only.
      "future_tool",
      "",
    ] {
      assert!(has_side_effects(name), "{name} must be a side effect");
    }
    for name in [
      "shell_poll",
      "view_image",
      "history_search",
      "history_read",
      "history_query",
      "skill_search",
      "memory_recall",
      "subagent_result",
    ] {
      assert!(!has_side_effects(name), "{name} must be read-only");
      assert!(is_read_only(name));
    }
  }

  #[test]
  fn read_only_denial_names_the_tool() {
    match denied_read_only("shell_start") {
      ToolOutcome::Failed(message) => assert_eq!(
        message,
        "denied: session is read-only (shell_start is a side-effect tool)"
      ),
      _ => panic!("expected a failure outcome"),
    }
  }

  #[test]
  fn preview_redacts_sensitive_keys_recursively() {
    let previewed = preview(&json!({
      "command": "deploy",
      "headers": {"Authorization": "Bearer sk-live-123", "api_key": "abc"},
      "nested": {"password": "hunter2", "safe": "value"},
    }));
    assert!(previewed.contains("[redacted]"), "{previewed}");
    assert!(!previewed.contains("sk-live-123"), "{previewed}");
    assert!(!previewed.contains("hunter2"), "{previewed}");
    assert!(!previewed.contains("abc"), "{previewed}");
    assert!(previewed.contains("deploy"), "{previewed}");
    assert!(previewed.contains("value"), "{previewed}");
  }

  #[test]
  fn redaction_masks_credentials_inside_free_text() {
    assert_eq!(
      mask_credentials("curl -H 'Authorization: Bearer sk-123' url"),
      "curl -H 'Authorization: Bearer [redacted]' url"
    );
    assert_eq!(mask_credentials("echo password=hunter2 now"), "echo password=[redacted] now");
    assert_eq!(mask_credentials("echo token=abc"), "echo token=[redacted]");
    // Unrelated text passes through untouched, including bearer-like words.
    assert_eq!(mask_credentials("echo hello world"), "echo hello world");
    assert_eq!(mask_credentials("bearer of bad news"), "bearer [redacted] bad news");
  }

  #[test]
  fn redact_preserves_non_sensitive_structure() {
    let value = redact(&json!({"a": [1, "two", {"b": true}], "c": null}));
    assert_eq!(value, json!({"a": [1, "two", {"b": true}], "c": null}));
  }

  #[test]
  fn previews_truncate_long_arguments() {
    assert_eq!(preview(&json!({"command": "ls"})), r#"{"command":"ls"}"#);
    let truncated = preview(&json!({"text": "x".repeat(300)}));
    assert_eq!(truncated, json!({"text": "x".repeat(300)}).to_string());
  }

  #[test]
  fn previews_keep_long_command_tails() {
    // A long command must stay reviewable past the old 200-char budget.
    let command = format!("echo start;{}", "y".repeat(5000));
    let previewed = preview(&json!({"command": command}));
    assert_eq!(previewed.chars().count(), super::PREVIEW_LIMIT + 1);
    assert!(previewed.ends_with('…'));
    assert!(previewed.contains("echo start;"));
    // Redaction happens before the cut: no secret survives anywhere.
    let secret = format!("echo start; token={}", "SECRET_VALUE".repeat(5000));
    let previewed = preview(&json!({"command": secret}));
    assert!(!previewed.contains("SECRET_VALUE"), "{previewed}");
  }
}

/// Executes the subagent and subagent_result tools against the shared manager.
async fn execute_subagent_tool(
  app: &Arc<App>,
  parent_session: &str,
  call: &ToolCall,
) -> ToolOutcome {
  use crate::server::subagents;
  match call.name.as_str() {
    "subagent" => {
      let Some(prompt) = call.arguments.get("prompt").and_then(Value::as_str) else {
        return ToolOutcome::Failed("prompt is required".into());
      };
      let description = call
        .arguments
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("delegated task");
      let agent_type = call
        .arguments
        .get("agent_type")
        .and_then(Value::as_str)
        .unwrap_or("general-purpose");
      let agent_id = app
        .subagents
        .spawn(parent_session.to_owned(), agent_type.to_owned(), prompt.to_owned(), description.to_owned())
        .await;
      let launch = subagents::launch(
        app.clone(),
        parent_session.to_owned(),
        agent_id.clone(),
        agent_type.to_owned(),
        prompt.to_owned(),
        description.to_owned(),
      )
      .await;
      match launch {
        Ok(value) => ToolOutcome::Success(value),
        Err(error) => {
          app.subagents.update_status(&agent_id, "failed").await;
          ToolOutcome::Failed(error)
        }
      }
    }
    "subagent_result" => {
      let Some(agent_id) = call.arguments.get("agent_id").and_then(Value::as_str) else {
        return ToolOutcome::Failed("agent_id is required".into());
      };
      match app.subagents.get(agent_id).await {
        Some(record) => {
          let value = json!({
            "agent_id": record.id,
            "status": record.status,
            "description": record.description,
            "agent_type": record.agent_type,
          });
          ToolOutcome::Success(value)
        }
        None => ToolOutcome::Failed(format!("unknown subagent: {agent_id}")),
      }
    }
    _ => ToolOutcome::Failed(format!("unknown subagent tool: {}", call.name)),
  }
}