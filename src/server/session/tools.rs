//! The built-in tools of a session: which optional ones it has (its switches), the executor that
//! runs them, and the shell its commands run under.
use super::SessionSlot;
use crate::server::{app::App, blobs, config::ShellSettings, error::{ApiError, blocking}};
use crate::{
  executor::{
    ExecutionControl,
    tool::ToolExecutor,
  },
  protocol::{ContentBlock, Message},
  session::{PermissionMode, Session, SessionConfig, ToolCall, ToolOutcome},
  tool::{
    ask_user::AskUserTool,
    history::HistoryTools,
    shell::{ShellCommand, ShellConfig, ShellTool},
    view_image::ViewImageTool,
    web_search::{Answer, SearchBackend, WebSearchTool},
  },
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Weak, atomic::{AtomicBool, Ordering}};

const HISTORY_TOOLS: [&str; 3] = ["history_search", "history_read", "history_query"];
const SHELL_TOOLS: [&str; 5] =
  ["shell_start", "shell_edit", "shell_poll", "shell_write", "shell_kill"];

/// The optional built-in tools of a session. History search and `view_image` are always there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSwitches {
  /// Commands in the session's working directory: `shell_start`, `shell_edit` and the rest.
  pub shell: bool,
  /// Questions to the user: `ask_user`.
  pub ask_user: bool,
  /// Whether `wish mcp` in the session's shell may reach the MCP servers. The model's request is
  /// the same either way, so switching it keeps the prompt cache.
  pub mcp: bool,
  /// Searches of the web: `web_search`, answered by the configured search providers.
  pub web_search: bool,
  /// Whether `wish skill` in the session's shell may read skills. Like `mcp`, it changes neither
  /// the tools nor the instructions, so switching it keeps the prompt cache.
  pub skills: bool,
  /// Whether `wish session` in the session's shell may see, make and message other sessions and
  /// groups. Like `mcp`, switching it keeps the prompt cache.
  pub sessions: bool,
}
impl ToolSwitches {
  /// These switches with a request's changes; what it leaves out stays as it is.
  pub fn apply_changes(self, changes: ToolChanges) -> Self {
    Self {
      shell: changes.shell.unwrap_or(self.shell),
      ask_user: changes.ask_user.unwrap_or(self.ask_user),
      mcp: changes.mcp.unwrap_or(self.mcp),
      web_search: changes.web_search.unwrap_or(self.web_search),
      skills: changes.skills.unwrap_or(self.skills),
      sessions: changes.sessions.unwrap_or(self.sessions),
    }
  }
  /// Whether a tool of this name runs under these switches.
  fn allow(self, name: &str) -> bool {
    match name {
      "view_image" => true,
      "ask_user" => self.ask_user,
      "web_search" => self.web_search,
      name if HISTORY_TOOLS.contains(&name) => true,
      name if SHELL_TOOLS.contains(&name) => self.shell,
      _ => false,
    }
  }
}
/// Switches a request sets, each optional.
#[derive(Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolChanges {
  pub shell: Option<bool>,
  pub ask_user: Option<bool>,
  pub mcp: Option<bool>,
  pub web_search: Option<bool>,
  pub skills: Option<bool>,
  pub sessions: Option<bool>,
}
impl From<ToolSwitches> for ToolChanges {
  /// Every switch set as these are.
  fn from(switches: ToolSwitches) -> Self {
    Self {
      shell: Some(switches.shell),
      ask_user: Some(switches.ask_user),
      mcp: Some(switches.mcp),
      web_search: Some(switches.web_search),
      skills: Some(switches.skills),
      sessions: Some(switches.sessions),
    }
  }
}

pub struct SessionTools {
  /// Started the first time the shell is switched on, and kept while it is off so background
  /// commands finish and report as usual.
  shell: tokio::sync::OnceCell<ShellTool>,
  shell_config: ShellConfig,
  shell_on: AtomicBool,
  ask_user_on: AtomicBool,
  pub ask_user: AskUserTool,
  web_search_on: AtomicBool,
  pub web_search: WebSearchTool,
  history: HistoryTools,
  slot: Weak<SessionSlot>,
  /// wish-enhanced: the application, which owns the shared web, MCP and
  /// subagent services a session's tools call into.
  pub(crate) app: Weak<App>,
  session_id: String,
  /// Workspace snapshots for undo and redo; present when snapshotting is enabled.
  pub snapshot: Option<crate::tool::snapshot::SnapshotTool>,
  /// Long-term memory recall over the shared ledger.
  pub memory: Option<crate::session::memory::MemoryRecallTool>,
}
impl SessionTools {
  pub fn new(
    slot: Weak<SessionSlot>,
    shell_config: ShellConfig,
    session: &Session,
    session_id: &str,
    app: Weak<App>,
    snapshot: Option<crate::tool::snapshot::SnapshotTool>,
    memory: Option<crate::session::memory::MemoryRecallTool>,
  ) -> Self {
    Self {
      shell: tokio::sync::OnceCell::new(),
      shell_config,
      shell_on: AtomicBool::new(false),
      ask_user_on: AtomicBool::new(false),
      ask_user: AskUserTool::new(questions_changed(slot.clone())),
      web_search_on: AtomicBool::new(false),
      web_search: WebSearchTool::new(search_backend(slot.clone()), session_id.to_owned()),
      history: HistoryTools::new(session),
      slot,
      app,
      session_id: session_id.to_owned(),
      snapshot,
      memory,
    }
  }
  /// The shell tool, once it has been switched on.
  pub fn shell(&self) -> Option<&ShellTool> {
    self.shell.get()
  }
  /// Applies the session's switches, starting the shell tool the first time it is on.
  pub async fn switch(&self, switches: ToolSwitches) -> Result<(), ApiError> {
    if switches.shell {
      self
        .shell
        .get_or_try_init(|| ShellTool::new(self.shell_config.clone()))
        .await
        .map_err(|error| ApiError::bad_request(format!("could not start the shell: {error}")))?;
    }
    self.shell_on.store(switches.shell, Ordering::Release);
    self.ask_user_on.store(switches.ask_user, Ordering::Release);
    self.web_search_on.store(switches.web_search, Ordering::Release);
    Ok(())
  }
  /// Whether subagent delegation is enabled in the configuration.
  pub fn subagents_enabled(&self) -> bool {
    self.app.upgrade().map(|app| app.subagents_enabled).unwrap_or(false)
  }
  /// An owning handle to the application, when it is still alive.
  pub fn app_handle(&self) -> Option<Arc<App>> {
    self.app.upgrade()
  }
  // MCP: upstream reaches servers through the `wish mcp` bridge in the shell
  // (see `McpHub`), not as namespaced model tools. The helpers below kept the
  // old namespaced shape and are gone with it; re-add them if model-visible MCP
  // tools come back.
  async fn execute_shell(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    let Some(shell) = self.shell().filter(|_| self.shell_on.load(Ordering::Acquire)) else {
      return ToolOutcome::Failed("shell is not enabled for this session".into());
    };
    let epoch = self.slot.upgrade().map(|slot| slot.get_schedule_epoch());
    let outcome = shell.execute(call, control).await;
    let ToolOutcome::Success(output) = &outcome else {
      return outcome;
    };
    if call.name != "shell_start" || output["process"]["status"] != "running" {
      return outcome;
    }
    let Some(id) = output["execution_id"].as_str() else { return outcome };
    let Some(app) = self.slot.upgrade().and_then(|slot| slot.app.upgrade()) else {
      return outcome;
    };
    // A command left running in the background reports its end as a message, which starts a run
    // unless the session was interrupted since this one began.
    let (shell, id, slot, command) =
      (shell.clone(), id.to_owned(), self.slot.clone(), call.arguments["command"].clone());
    let owner = app.clone();
    app.lifecycle.tasks.spawn(async move {
      let result = tokio::select! {
        _ = owner.lifecycle.stop.cancelled() => return,
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
      let Some(slot) = slot.upgrade().filter(|slot| slot.require_live().is_ok()) else { return };
      if let Err(error) = slot.enqueue(message).await {
        eprintln!("background notification: {error}");
        return;
      }
      if slot.require_live().is_ok() {
        let _ = owner.events.send(json!({"type":"session_changed","id":slot.get_descriptor().id}));
        if epoch == Some(slot.get_schedule_epoch()) {
          slot.schedule(&owner);
        }
      }
    });
    outcome
  }
  async fn view_image(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    let outcome = ViewImageTool.execute(call, control).await;
    let ToolOutcome::SuccessWithInput { mut output, mut input } = outcome else {
      return outcome;
    };
    let Some(slot) = self.slot.upgrade() else {
      return ToolOutcome::Failed("the session is closed".into());
    };
    for block in &input {
      if let ContentBlock::Image { data_base64, .. } = block {
        let (directory, data) = (slot.image_dir.clone(), data_base64.clone());
        let saved =
          blocking(move || blobs::save_image(&directory, &data).map_err(ApiError::internal)).await;
        match saved {
          Ok(path) => output["session_path"] = json!(path),
          Err(error) => return ToolOutcome::Failed(error.to_string()),
        }
      }
    }
    input.retain(|block| matches!(block, ContentBlock::Image { .. }));
    ToolOutcome::SuccessWithInput { output, input }
  }
}
/// Searches with the application's search providers as they stand at the moment of the search.
fn search_backend(slot: Weak<SessionSlot>) -> SearchBackend {
  Arc::new(move |query| {
    let app = slot.upgrade().and_then(|slot| slot.app.upgrade());
    Box::pin(async move {
      let app = app.ok_or_else(|| vec!["the server is shutting down".to_owned()])?;
      let found = app.search.search(&app.get_providers(), &query).await?;
      Ok(Answer { provider: found.provider, name: found.name, results: found.results })
    })
  })
}
/// Publishes the session's open questions whenever they change: its index record and a change
/// notice.
fn questions_changed(slot: Weak<SessionSlot>) -> impl Fn() + Send + Sync + 'static {
  move || {
    let Some(slot) = slot.upgrade() else { return };
    let tasks = slot.tasks.clone();
    tasks.spawn(async move {
      let _ = slot.persist_index();
    });
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
        "time": crate::utils::time::Timestamp::now().0,
        "args": preview(&call.arguments),
      }));
    }
    outcome
  }
}

impl SessionTools {
  async fn dispatch(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    match call.name.as_str() {
      "view_image" => self.view_image(call, control).await,
      "ask_user" => {
        if !self.ask_user_on.load(Ordering::Acquire) {
          return ToolOutcome::Failed("ask_user is not enabled for this session".into());
        }
        self.ask_user.execute(call, control).await
      }
      "web_search" => {
        if !self.web_search_on.load(Ordering::Acquire) {
          return ToolOutcome::Failed("web_search is not enabled for this session".into());
        }
        self.web_search.execute(call, control).await
      }
      "snapshot_checkpoint" | "snapshot_undo" | "snapshot_redo" => {
        let Some(snapshot) = &self.snapshot else {
          return ToolOutcome::Failed("workspace snapshots are disabled for this session".into());
        };
        snapshot.execute(call, control).await
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
      // MCP servers are reached through the shell bridge (`wish mcp`), not as
      // namespaced model tools; see `McpHub`.
      name if HISTORY_TOOLS.contains(&name) => self.history.execute(call, control).await,
      name if SHELL_TOOLS.contains(&name) => self.execute_shell(call, control).await,
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
  use crate::session::ToolOutcome;
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
impl SessionSlot {
  /// The session config with the tool list its switches give it. A tool the config names that the
  /// session could not run is refused.
  pub fn configure_tools(&self, mut config: SessionConfig) -> Result<SessionConfig, ApiError> {
    let switches = self.descriptor.read().unwrap().tools;
    if let Some(tool) = config.tools.iter().find(|tool| !switches.allow(&tool.name)) {
      return Err(ApiError::bad_request(format!("no executor for tool {}", tool.name)));
    }
    config.tools.clear();
    config.tools.extend(self.tools.history.get_specifications());
    config.tools.push(ViewImageTool.get_specification());
    if switches.shell
      && let Some(shell) = self.tools.shell()
    {
      config.tools.extend(shell.get_specifications());
    }
    if switches.ask_user {
      config.tools.push(self.tools.ask_user.get_specification());
    }
    if switches.web_search {
      config.tools.push(self.tools.web_search.get_specification());
    }
    // wish-enhanced: workspace snapshots, long-term memory recall and
    // subagent delegation, each present only when the application enabled it.
    if let Some(snapshot) = &self.tools.snapshot {
      config.tools.extend(snapshot.get_specifications());
    }
    if let Some(memory) = &self.tools.memory {
      config.tools.push(memory.get_specification());
    }
    if self.tools.subagents_enabled()
      && let Some(app) = self.tools.app_handle()
    {
      config.tools.extend(crate::server::subagents::get_specifications(&app.subagents));
    }
    Ok(config)
  }
  /// Turns optional tools on or off; the caller then reinstalls the session's tool list.
  pub async fn switch_tools(&self, changes: ToolChanges) -> Result<(), ApiError> {
    let previous = self.descriptor.read().unwrap().tools;
    let next = previous.apply_changes(changes);
    self.tools.switch(next).await?;
    self.descriptor.write().unwrap().tools = next;
    if previous.mcp
      && !next.mcp
      && let Some(app) = self.app.upgrade()
    {
      app.mcp.close_session(&self.get_descriptor().id);
    }
    Ok(())
  }
  /// Applies the application's new shell unless this session has its own.
  pub fn follow_global_shell(&self, global: &ShellCommand) {
    if self.descriptor.read().unwrap().shell_override.is_none() {
      *self.effective_shell.write().unwrap() = global.clone();
    }
  }
  /// Gives this session its own shell, or with None returns it to the application's.
  pub fn set_shell(
    &self,
    settings: Option<ShellSettings>,
    global: &ShellCommand,
  ) -> Result<(), ApiError> {
    if !self.descriptor.read().unwrap().tools.shell {
      return Err(ApiError::bad_request("this session has no shell tool"));
    }
    let next = match &settings {
      Some(settings) => settings.resolve()?,
      None => global.clone(),
    };
    *self.effective_shell.write().unwrap() = next;
    self.descriptor.write().unwrap().shell_override = settings;
    self.persist_index()
  }
}
