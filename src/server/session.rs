//! Open sessions. A `SessionSlot` holds one while the server runs: the engine's session behind a
//! lock, a sender and a reader beside it, and the descriptor - the application's own record of the session:
//! its name, provider, working directory and tool switches. The descriptor and the session's
//! status together are its index record, which the session list reads.
//!
//! - `run` starts, schedules and interrupts operations, and runs them;
//! - `status` keeps the status the API reports, and turns engine events into web events;
//! - `selection` changes provider and model, now or at an operation's next boundary;
//! - `tools` holds the built-in tools and the switches that decide which a session has.
mod live;
mod run;
pub mod selection;
mod status;
mod tools;

pub use run::Operation;
pub use status::web_event;
pub use tools::{ToolChanges, ToolSwitches};

use crate::server::{
  app::App,
  config::ShellSettings,
  error::{ApiError, blocking},
  management::ManagementStore,
};
use crate::{
  executor::ExecutionControl,
  protocol::Message,
  session::{
    EntryId, PermissionMode, Session, SessionConfig, SessionReader, SessionSender,
  },
  tool::shell::{ShellCommand, ShellConfig},
  utils::time::Timestamp,
};
use axum::http::HeaderValue;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use status::SessionStatus;
use std::{
  path::PathBuf,
  sync::{
    Arc, Mutex, RwLock, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
  },
};
use tokio::sync::{Mutex as AsyncMutex, broadcast};
use tools::SessionTools;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
  /// A provider and config chosen while an operation ran, applied at its next boundary.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub pending_selection: Option<selection::PendingSelection>,
  /// The session that made this one with `wish session create`, which may configure and delete
  /// it; absent for one the user made.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub created_by: Option<String>,
  pub name: String,
  pub updated_at: u64,
  /// Advanced by every edit through `edit_descriptor`, which an `If-Match` header names.
  pub revision: u64,
  pub id: String,
  pub provider: String,
  pub cwd: PathBuf,
  /// Which optional built-in tools the session has.
  pub tools: ToolSwitches,
  /// This session's own shell; absent while it follows the application's.
  #[serde(rename = "shell_command", default, skip_serializing_if = "Option::is_none")]
  pub shell_override: Option<ShellSettings>,
  pub created_at: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSession {
  /// The session making this one through `wish session`, which may then configure and delete it.
  #[serde(skip)]
  pub created_by: Option<String>,
  #[serde(default)]
  pub initial_messages: Vec<Message>,
  /// Where each initial message came from, when it came from another session (a fork); missing
  /// origins are `Imported`.
  #[serde(skip)]
  pub initial_origins: Vec<crate::session::EntryOrigin>,
  #[serde(default)]
  pub name: String,
  pub provider: String,
  pub config: SessionConfig,
  pub cwd: PathBuf,
  /// Optional tools to switch from the API defaults: no shell and no web search, `ask_user` and
  /// MCP on.
  #[serde(default)]
  pub tools: ToolChanges,
  #[serde(default)]
  pub metadata: Value,
  /// The folder the session goes in; none for the root of the list.
  #[serde(default)]
  pub folder: Option<String>,
}

pub struct SessionSlot {
  descriptor: RwLock<Descriptor>,
  management: Arc<ManagementStore>,
  /// The application's events, where the session list hears of changes.
  global_events: broadcast::Sender<Value>,
  /// Advanced by every interrupt: a run scheduled before one does not start after it.
  schedule_epoch: AtomicU64,
  deleted: AtomicBool,
  pub session: Arc<AsyncMutex<Session>>,
  /// Serializes descriptor edits, which never wait for an operation.
  pub update_lock: Arc<AsyncMutex<()>>,
  /// Queues input while an operation holds the session.
  pub sender: SessionSender,
  /// Reads the session's history and lists while an operation holds it.
  pub reader: SessionReader,
  pub tools: SessionTools,
  /// The command this session's shell tool starts, whether or not the tool is on.
  effective_shell: Arc<RwLock<ShellCommand>>,
  /// The session's blobs, where its images are saved.
  image_dir: PathBuf,
  /// What this session's shell shows the MCP bridge; made anew each time the session opens, since
  /// its shells do not outlive the process.
  pub bridge_token: String,
  tasks: tokio_util::task::TaskTracker,
  app: Weak<App>,
  /// This session's live events, for `GET /sessions/{id}/events`.
  pub events: broadcast::Sender<Value>,
  live: Mutex<live::LivePreview>,
  status: Mutex<SessionStatus>,
  /// The running operation's, while one runs.
  control: Mutex<Option<ExecutionControl>>,
  /// Advanced by every automatic run. An ask-mode approval is bound to the run
  /// that asked for it, so an interrupt invalidates the card it was waiting on.
  pub auto_run_generation: AtomicU64,
  /// The live permission mode; it gates side-effect tools even mid-run.
  pub permission: Mutex<PermissionMode>,
  /// A mode change requested while a run owned the session; persisted at run end.
  pending_permission: Mutex<Option<PermissionMode>>,
  /// Ask-mode approvals awaiting a user decision, with a redacted summary.
  /// Kept beside the status so a UI refresh cannot lose a pending card.
  pending_approvals: Mutex<Vec<Value>>,
}
impl SessionSlot {
  pub async fn open(
    app: &Arc<App>,
    descriptor: Descriptor,
    mut session: Session,
  ) -> Result<Arc<Self>, ApiError> {
    if !session.get_state().is_stable() {
      session.settle_interrupted().map_err(ApiError::internal)?;
    }
    // Sessions persisted before auto-compaction was on by default carry
    // `compaction: None`, which the executor reads as "off". Fill it from the
    // application defaults so those sessions compact like newly created ones.
    if session.get_config().compaction.is_none() {
      let fill = app.config_file.lock().await.config.defaults.compaction.clone();
      if let Some(fill) = fill {
        let mut config = session.get_config().clone();
        config.compaction = Some(fill);
        session.set_config(config).map_err(ApiError::internal)?;
      }
    }
    // A session's own shell wins; otherwise it starts from the application's and
    // follows later saves (see `follow_global_shell`). An override that no longer
    // resolves (the program was removed) falls back to the application's.
    let own = descriptor.shell_override.as_ref().and_then(|settings| settings.resolve().ok());
    let effective_shell =
      Arc::new(RwLock::new(own.unwrap_or_else(|| app.shell.read().unwrap().clone())));
    let mut shell_config = ShellConfig::new(&descriptor.cwd, app.data_dir.shell(&descriptor.id));
    shell_config.command = Arc::clone(&effective_shell);
    let bridge_token =
      format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    // Set whatever the switches say: the bridge checks them, and the environment stays as it is.
    shell_config.env.extend(app.bridge.shell_environment(&descriptor.id, &bridge_token));
    let image_dir =
      std::path::absolute(app.data_dir.blobs(&descriptor.id)).map_err(ApiError::internal)?;
    let switches = descriptor.tools;
    let status = Mutex::new(SessionStatus::read(&session));
    let permission = Mutex::new(session.get_config().permission);
    // wish-enhanced: the optional tools the application configuration asks for.
    // Snapshots follow `workspace_history` (true/false/auto); skills and memory
    // follow their own switches.
    let snapshot_tool = {
      let enabled = app.config_file.lock().await.config.workspace_history.enabled.clone();
      let want = match enabled.as_str() {
        "true" => true,
        "false" => false,
        _ => crate::tool::snapshot::looks_like_project(&descriptor.cwd),
      };
      if want {
        let git_dir =
          app.data_dir.root().join("snapshots").join(&descriptor.id).join("repo.git");
        Some(crate::tool::snapshot::SnapshotTool::new(descriptor.cwd.clone(), git_dir))
      } else {
        None
      }
    };
    let memory = app
      .memory
      .clone()
      .map(|store| crate::session::memory::MemoryRecallTool { store });
    let (events, _) = broadcast::channel(256);
    let slot = Arc::new_cyclic(|slot| Self {
      tools: SessionTools::new(
        slot.clone(),
        shell_config,
        &session,
        &descriptor.id,
        Arc::downgrade(app),
        snapshot_tool,
        memory,
      ),
      descriptor: RwLock::new(descriptor),
      management: app.management.clone(),
      global_events: app.events.clone(),
      schedule_epoch: AtomicU64::new(0),
      deleted: AtomicBool::new(false),
      sender: session.create_sender(),
      reader: session.reader().clone(),
      session: Arc::new(AsyncMutex::new(session)),
      update_lock: Arc::new(AsyncMutex::new(())),
      effective_shell,
      image_dir,
      bridge_token,
      tasks: app.lifecycle.tasks.clone(),
      app: Arc::downgrade(app),
      events,
      live: Mutex::new(live::LivePreview::default()),
      status,
      control: Mutex::new(None),
      auto_run_generation: AtomicU64::new(0),
      permission,
      pending_permission: Mutex::new(None),
      // A reopen (service restart) never inherits pending approvals: their
      // waiters died with the old process, so no card is actionable any more.
      pending_approvals: Mutex::new(Vec::new()),
    });
    slot.tools.switch(switches).await?;
    Ok(slot)
  }
  pub fn get_descriptor(&self) -> Descriptor {
    self.descriptor.read().unwrap().clone()
  }
  /// Edits the descriptor, checking the revision an `If-Match` header names. The edit works on a
  /// copy, which the session takes - a revision on, stamped now - once the index holds it.
  pub fn edit_descriptor(
    &self,
    if_match: Option<&HeaderValue>,
    edit: impl FnOnce(&mut Descriptor) -> Result<(), ApiError>,
  ) -> Result<(), ApiError> {
    let mut descriptor = self.descriptor.write().unwrap();
    expect_revision(if_match, descriptor.revision)?;
    let mut next = descriptor.clone();
    edit(&mut next)?;
    next.revision += 1;
    next.updated_at = Timestamp::now().0;
    self.management.save(&self.index_record(&next))?;
    *descriptor = next;
    drop(descriptor);
    self.announce_change();
    Ok(())
  }
  /// Refuses an edit made against a revision the descriptor has moved past.
  pub fn check_revision(&self, if_match: Option<&HeaderValue>) -> Result<(), ApiError> {
    expect_revision(if_match, self.descriptor.read().unwrap().revision)
  }
  /// Marks the session as just used - a user message or answer arrived - and saves the index, so
  /// the session list, ordered by `updated_at`, brings it to the top at once.
  pub fn touch(&self) -> Result<(), ApiError> {
    self.descriptor.write().unwrap().updated_at = Timestamp::now().0;
    self.persist_index()
  }
  /// The session as the API shows it.
  pub fn describe(&self) -> Value {
    let mut record = self.index_record(&self.get_descriptor());
    preview_pending_selection(&mut record);
    record
  }
  /// The session as the index keeps it: the provider it actually uses, which a restart may still
  /// need to translate an encrypted compaction item before a pending selection applies, and the
  /// status as the API shows it.
  fn index_record(&self, descriptor: &Descriptor) -> Value {
    json!({"session":descriptor,"status":self.view_status(descriptor)})
  }
  /// Saves the index record, and tells the session list.
  pub fn persist_index(&self) -> Result<(), ApiError> {
    self.require_live()?;
    self.management.save(&self.index_record(&self.get_descriptor()))?;
    self.announce_change();
    Ok(())
  }
  fn announce_change(&self) {
    let _ =
      self.global_events.send(json!({"type":"session_changed","id":self.get_descriptor().id}));
  }
  /// Takes a change made to the session: refreshes its status and index record, and returns the
  /// session as the API shows it.
  pub fn publish(&self, session: &Session) -> Result<Value, ApiError> {
    self.refresh_status(session);
    self.persist_index()?;
    Ok(self.describe())
  }
  /// Queues a message for the session's next run.
  pub async fn enqueue(&self, message: Message) -> Result<EntryId, ApiError> {
    let sender = self.sender.clone();
    blocking(move || Ok(sender.enqueue_message(message)?)).await
  }
  pub fn require_live(&self) -> Result<(), ApiError> {
    if self.deleted.load(Ordering::Acquire) { Err(ApiError::not_found()) } else { Ok(()) }
  }

  // --- permission mode and ask-mode approvals (wish-enhanced) ---

  /// Refreshes the status from a session no operation holds, and follows the
  /// permission mode a config edit may have brought with it.
  pub fn update_snapshot(&self, session: &Session) {
    self.refresh_status(session);
    let next = session.get_config().permission;
    if let Ok(mut current) = self.permission.try_lock() {
      if *current != next && next != PermissionMode::Ask {
        // Leaving ask voids whatever is pending: the waiting call is aborted,
        // never silently executed without an explicit decision.
        self.cancel_pending_approvals();
      }
      *current = next;
    }
  }
  /// Switches the permission mode. Runs immediately when the session is idle,
  /// and is applied by the running operation at its next boundary otherwise.
  pub fn set_permission(&self, mode: PermissionMode) -> Result<(), ApiError> {
    self.require_live()?;
    let previous = *self.permission.lock().unwrap();
    *self.permission.lock().unwrap() = mode;
    if previous != mode && mode != PermissionMode::Ask {
      self.cancel_pending_approvals();
    }
    match self.session.clone().try_lock_owned() {
      Ok(mut session) => {
        session.set_permission(mode)?;
        self.update_snapshot(&session);
        self.persist_index()?;
      }
      Err(_) => {
        // A run owns the session: remember the mode and apply it when it ends.
        *self.pending_permission.lock().unwrap() = Some(mode);
      }
    }
    Ok(())
  }
  /// The mode a run should take over, once it releases the session.
  pub fn take_pending_permission(&self) -> Option<PermissionMode> {
    self.pending_permission.lock().unwrap().take()
  }
  /// The live permission mode, read by the tool executor before a side effect.
  pub fn permission(&self) -> PermissionMode {
    *self.permission.lock().unwrap()
  }
  /// Records an approval the agent is waiting for, so a card survives a refresh.
  pub fn note_pending_approval(
    &self,
    call_id: &str,
    name: &str,
    summary: &str,
    generation: u64,
  ) {
    {
      let mut live = self.live.lock().unwrap();
      live.note_approval(call_id, name, summary, generation);
    }
    self.pending_approvals.lock().unwrap().retain(|entry| {
      entry["call_id"].as_str() != Some(call_id) || entry["generation"].as_u64() != Some(generation)
    });
    self.pending_approvals.lock().unwrap().push(json!({
      "call_id": call_id,
      "name": name,
      "generation": generation,
      "summary": summary,
    }));
  }
  /// Retires an approval: the decision landed, or the call was cancelled.
  pub fn clear_pending_approval(&self, call_id: &str, generation: u64) {
    let removed = {
      let mut pending = self.pending_approvals.lock().unwrap();
      let before = pending.len();
      pending.retain(|entry| {
        entry["call_id"].as_str() != Some(call_id)
          || entry["generation"].as_u64() != Some(generation)
      });
      before != pending.len()
    };
    {
      let mut live = self.live.lock().unwrap();
      live.retire_approval(call_id, generation);
    }
    if removed {
      let _ = self.events.send(json!({"type":"approval_closed","call_id":call_id,"generation":generation}));
    }
  }
  /// The approvals still waiting, as the API and the UI show them.
  pub fn list_pending_approvals(&self) -> Vec<Value> {
    self.pending_approvals.lock().unwrap().clone()
  }
  /// Abandons every approval of this session: the run they belonged to is over.
  fn cancel_pending_approvals(&self) {
    if let Some(app) = self.app.upgrade() {
      app.cancel_approvals(&self.get_descriptor().id, None);
    }
  }
  /// Marks the session deleted: from now on it is not found.
  pub fn mark_deleted(&self) {
    self.deleted.store(true, Ordering::Release);
  }
}

/// Shows a pending model selection in an index record as made: its provider in place of the
/// session's. The status shows its config already.
pub fn preview_pending_selection(record: &mut Value) {
  if let Some(provider) = record.pointer("/session/pending_selection/provider").cloned() {
    record["session"]["provider"] = provider;
  }
}

fn expect_revision(if_match: Option<&HeaderValue>, revision: u64) -> Result<(), ApiError> {
  match if_match {
    Some(value) if value.to_str().ok() != Some(revision.to_string().as_str()) => {
      Err(ApiError::conflict("session changed; reload before saving"))
    }
    _ => Ok(()),
  }
}
