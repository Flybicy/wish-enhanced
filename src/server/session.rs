mod live;
pub mod selection;
mod tools;
use crate::server::{config::ShellSettings, error::ApiError, provider::Provider};
use crate::{
  executor::{self, ExecutionControl},
  protocol::StreamEvent,
  session::{
    Entry, EntryId, EntryOrigin, Generation, HistoryReader, RunOutcome, Session, SessionConfig,
    SessionEvent, SessionHandle,
    statistics::{ModelCallPurpose, ModelCallRecord, ModelCallStatus},
  },
  storage::ReadList,
  tool::{
    shell::{ShellCommand, ShellConfig, ShellTool},
    view_image::ViewImageTool,
  },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
  path::PathBuf,
  sync::{Arc, Mutex, RwLock},
};
use tokio::sync::{Mutex as AsyncMutex, broadcast};
use tools::SessionTools;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub pending_selection: Option<selection::PendingSelection>,
  pub name: String,
  pub updated_at: u64,
  pub revision: u64,
  pub id: String,
  pub provider: String,
  pub cwd: PathBuf,
  pub shell: bool,
  /// This session's own shell; absent while it follows the application's.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub shell_command: Option<ShellSettings>,
  pub created_at: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSession {
  #[serde(default)]
  pub initial_messages: Vec<crate::protocol::Message>,
  #[serde(default)]
  pub name: String,
  pub provider: String,
  pub config: SessionConfig,
  pub cwd: PathBuf,
  #[serde(default)]
  pub shell: bool,
  #[serde(default)]
  pub metadata: Value,
}

pub struct SessionSlot {
  pub descriptor: RwLock<Descriptor>,
  pub index: Arc<crate::server::management::ManagementStore>,
  pub global_events: broadcast::Sender<Value>,
  pub auto_run_generation: std::sync::atomic::AtomicU64,
  pub deleted: std::sync::atomic::AtomicBool,
  pub session: Arc<AsyncMutex<Session>>,
  pub selection_edit: Arc<AsyncMutex<()>>,
  pub handle: SessionHandle,
  pub history: HistoryReader,
  pub entries: ReadList<Entry>,
  pub generations: ReadList<Generation>,
  pub calls: ReadList<ModelCallRecord>,
  /// Settled folds behind the snapshot stats block; see stats_value.
  stats: Mutex<StatsMemo>,
  pub queue: Mutex<ReadList<EntryId>>,
  pub tools: SessionTools,
  /// The command this session's shell tool starts; None without a shell tool.
  pub shell_command: Option<Arc<RwLock<ShellCommand>>>,
  pub image_dir: PathBuf,
  tasks: tokio_util::task::TaskTracker,
  app: std::sync::Weak<crate::server::app::App>,
  pub events: broadcast::Sender<Value>,
  live: Mutex<live::LivePreview>,
  pub status: Mutex<Value>,
  pub control: Mutex<Option<ExecutionControl>>,
}
impl SessionSlot {
  pub async fn open(
    descriptor: Descriptor,
    mut session: Session,
    data_dir: PathBuf,
    index: Arc<crate::server::management::ManagementStore>,
    global_events: broadcast::Sender<Value>,
    app: std::sync::Weak<crate::server::app::App>,
  ) -> Result<Arc<Self>, ApiError> {
    if !session.get_state().is_stable() {
      session.settle_interrupted().map_err(ApiError::internal)?;
    }
    // Sessions persisted before auto-compaction was on by default carry
    // `compaction: None`, which the executor reads as "off". Fill it from the
    // application defaults so those sessions compact like newly created ones.
    if let Some(app) = app.upgrade() {
      if session.get_config().compaction.is_none() {
        let fill = app.configuration.lock().await.config.defaults.compaction.clone();
        if let Some(fill) = fill {
          let mut config = session.get_config().clone();
          config.compaction = Some(fill);
          session.set_config(config).map_err(ApiError::internal)?;
        }
      }
    }
    // A session's own shell wins; otherwise it starts from the application's and
    // follows later saves (see `follow_global_shell`). An override that no longer
    // resolves (the program was removed) falls back to the application's.
    let shell_command = descriptor.shell.then(|| {
      let global = app.upgrade().map_or_else(ShellCommand::platform_default, |app| {
        app.shell.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
      });
      let own = descriptor.shell_command.as_ref().and_then(|settings| settings.resolve().ok());
      Arc::new(RwLock::new(own.unwrap_or(global)))
    });
    let shell = if let Some(command) = &shell_command {
      let mut config =
        ShellConfig::new(&descriptor.cwd, data_dir.join("shell").join(&descriptor.id));
      config.command = Arc::clone(command);
      Some(ShellTool::new(config).await.map_err(ApiError::internal)?)
    } else {
      None
    };
    let image_dir = std::path::absolute(data_dir.join("blobs").join(&descriptor.id))
      .map_err(ApiError::internal)?;
    // Workspace snapshots follow the application setting: true, false, or "auto".
    let snapshot_tool = {
      let enabled = match app.upgrade() {
        Some(app) => {
          let configuration = app.configuration.lock().await;
          configuration.config.workspace_history.enabled.clone()
        }
        None => "auto".into(),
      };
      let want = match enabled.as_str() {
        "true" => true,
        "false" => false,
        _ => crate::tool::snapshot::looks_like_project(&descriptor.cwd),
      };
      if want {
        let git_dir = data_dir.join("snapshots").join(&descriptor.id).join("repo.git");
        Some(crate::tool::snapshot::SnapshotTool::new(descriptor.cwd.clone(), git_dir))
      } else {
        None
      }
    };
    let tasks = app.upgrade().expect("session application is alive").tasks.clone();
    let skills = match app.upgrade() {
      Some(app) => {
        let (enabled, extra) = {
          let configuration = app.configuration.lock().await;
          (configuration.config.skills.enabled, configuration.config.skills.dirs.clone())
        };
        if enabled {
          let mut dirs = vec![app.data_dir.join("skills")];
          dirs.extend(extra);
          Some(crate::server::skills::SkillSearchTool::new(dirs))
        } else {
          None
        }
      }
      _ => None,
    };
    let memory = match app.upgrade() {
      Some(app) => app.memory.clone().map(|store| crate::session::memory::MemoryRecallTool { store }),
      None => None,
    };
    let tools =
      SessionTools::new(shell, &session, app.clone(), descriptor.id.clone(), image_dir.clone(), snapshot_tool, skills, memory);
    let entries = session.get_entries();
    let calls = session.get_model_calls();
    let mut stats_memo = StatsMemo::default();
    let status = Mutex::new(snapshot(&session, stats_value(&entries, &calls, &mut stats_memo)));
    let (events, _) = broadcast::channel(256);
    Ok(Arc::new(Self {
      descriptor: RwLock::new(descriptor),
      index,
      global_events,
      auto_run_generation: std::sync::atomic::AtomicU64::new(0),
      deleted: std::sync::atomic::AtomicBool::new(false),
      handle: session.create_handle(),
      history: session.create_history_reader(),
      entries,
      generations: session.get_generations(),
      calls,
      stats: Mutex::new(stats_memo),
      queue: Mutex::new(session.get_message_queue()),
      session: Arc::new(AsyncMutex::new(session)),
      selection_edit: Arc::new(AsyncMutex::new(())),
      tools,
      shell_command,
      image_dir,
      tasks,
      app,
      events,
      live: Mutex::new(live::LivePreview::default()),
      status,
      control: Mutex::new(None),
    }))
  }
  pub fn configure_tools(&self, mut config: SessionConfig) -> Result<SessionConfig, ApiError> {
    for tool in &config.tools {
      let is_valid = match tool.name.as_str() {
        "view_image" | "history_search" | "history_read" | "history_query" => true,
        "shell_start" | "shell_edit" | "shell_poll" | "shell_write" | "shell_kill" => {
          self.tools.shell.is_some()
        }
        "web_search" | "fetch_content" | "get_search_content" => {
          self.tools.web_enabled()
        }
        "snapshot_checkpoint" | "snapshot_undo" | "snapshot_redo" => self.tools.snapshot.is_some(),
        "skill_search" => self.tools.skills.is_some(),
        "memory_recall" => self.tools.memory.is_some(),
        "subagent" | "subagent_result" => self.tools.subagents_enabled(),
        name if name.starts_with("mcp_") => {
          let rest = &name["mcp_".len()..];
          match rest.split_once('_') {
            Some((server, _)) => self.tools.mcp_registered(server),
            None => false,
          }
        }
        _ => false,
      };
      if !is_valid {
        return Err(ApiError::bad_request(format!("no executor for tool {}", tool.name)));
      }
    }
    config.tools.clear();
    config.tools.extend(self.tools.get_history_specifications());
    config.tools.push(ViewImageTool.get_specification());
    if let Some(shell) = &self.tools.shell {
      config.tools.extend(shell.get_specifications());
    }
    if let Some(web) = self.tools.web_specifications() {
      config.tools.extend(web);
    }
    if let Some(snapshot) = &self.tools.snapshot {
      config.tools.extend(snapshot.get_specifications());
    }
    if let Some(skills) = &self.tools.skills {
      config.tools.push(skills.get_specification());
    }
    if let Some(memory) = &self.tools.memory {
      config.tools.push(memory.get_specification());
    }
    if self.tools.subagents_enabled() {
      if let Some(app) = self.tools.app_handle() {
        config.tools.extend(crate::server::subagents::get_specifications(&app.subagents));
      }
    }
    config.tools.extend(self.tools.mcp_specifications());
    Ok(config)
  }
  pub fn describe(&self) -> Value {
    {
      let mut status = self.status.lock().unwrap().clone();
      status["queue_count"] = json!(
        self
          .queue
          .lock()
          .unwrap()
          .len()
          .unwrap_or(0)
          .saturating_sub(status["queue_head"].as_u64().unwrap_or(0))
      );
      let mut descriptor = self.get_descriptor();
      if let Some(pending) = &descriptor.pending_selection {
        descriptor.provider = pending.provider.clone();
        status["config"] = json!(pending.config);
        status["selection_pending"] = json!(true);
      }
      json!({"session":descriptor,"status":status})
    }
  }
  pub fn get_descriptor(&self) -> Descriptor {
    self.descriptor.read().unwrap().clone()
  }
  /// Applies the application's new shell unless this session has its own.
  pub fn follow_global_shell(&self, global: &ShellCommand) {
    if self.descriptor.read().unwrap().shell_command.is_none()
      && let Some(command) = &self.shell_command
    {
      *command.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = global.clone();
    }
  }
  /// Gives this session its own shell, or with None returns it to the application's.
  pub fn set_shell(
    &self,
    settings: Option<ShellSettings>,
    global: &ShellCommand,
  ) -> Result<(), ApiError> {
    let Some(command) = &self.shell_command else {
      return Err(ApiError::bad_request("this session has no shell tool"));
    };
    let next = match &settings {
      Some(settings) => settings.resolve()?,
      None => global.clone(),
    };
    *command.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
    self.descriptor.write().unwrap().shell_command = settings;
    self.persist_index()
  }
  pub fn require_live(&self) -> Result<(), ApiError> {
    if self.deleted.load(std::sync::atomic::Ordering::Acquire) {
      Err(ApiError::not_found())
    } else {
      Ok(())
    }
  }
  pub fn persist_index(&self) -> Result<(), ApiError> {
    self.require_live()?;
    let mut record = self.describe();
    // `describe` previews a pending provider to the UI. Persist the actual provider so a restart
    // can still ask it to translate an encrypted compaction item before applying the selection.
    record["session"] = json!(self.get_descriptor());
    self.index.save(&record)?;
    let _ =
      self.global_events.send(json!({"type":"session_changed","id":self.get_descriptor().id}));
    Ok(())
  }
  pub fn update_snapshot(&self, session: &Session) {
    *self.queue.lock().unwrap() = session.get_message_queue();
    *self.status.lock().unwrap() = self.refresh_status(session);
  }
  /// Rebuilds the status with fresh stats folds; the memo keeps each read cheap.
  fn refresh_status(&self, session: &Session) -> Value {
    let mut memo = self.stats.lock().unwrap();
    snapshot(session, stats_value(&self.entries, &self.calls, &mut memo))
  }
  pub fn interrupt(&self) -> bool {
    self.auto_run_generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if let Some(control) = self.control.lock().unwrap().as_ref() {
      control.cancel();
      true
    } else {
      false
    }
  }
  pub fn observe(&self, event: &SessionEvent) {
    if let SessionEvent::InputsConsumed { queue_end, .. } = event {
      self.status.lock().unwrap()["queue_head"] = json!(queue_end);
    }
    if let SessionEvent::StateChanged { to, .. } = event {
      self.status.lock().unwrap()["phase"] = json!(to);
      if let Err(error) = self.persist_index() {
        eprintln!("session index: {error}");
      }
    }
    if let SessionEvent::CompactionSummaryStarted { .. } = event {
      self.status.lock().unwrap()["standby_preparing"] = json!(true);
    }
    if matches!(
      event,
      SessionEvent::CompactionSummary { .. } | SessionEvent::CompactionSummaryFailed { .. }
    ) {
      self.status.lock().unwrap()["standby_preparing"] = json!(false);
    }
    if let SessionEvent::ContextCompacted { .. } = event {
      self.status.lock().unwrap()["standby_preparing"] = json!(false);
    }
    let mut live = self.live.lock().unwrap();
    live.observe(event);
    if let Some(event) = web_event(event) {
      let _ = self.events.send(json!({"type":"session_event","event":event,"revision":live.revision}));
    }
  }
  pub fn subscribe_live(&self) -> (broadcast::Receiver<Value>, Value) {
    // Subscription and snapshot share the publication lock: no gap or duplicate deltas.
    let live = self.live.lock().unwrap();
    (self.events.subscribe(), live.snapshot(self.describe()))
  }
  pub async fn execute(
    self: Arc<Self>,
    provider: Arc<Provider>,
    provider_id: String,
    mut session: tokio::sync::OwnedMutexGuard<Session>,
    control: ExecutionControl,
    compact: bool,
  ) {
    let model = selection::SwitchingModel::new(self.make_model(provider, provider_id));
    let result = if compact {
      let prepared = match session.get_history().len() {
        Ok(mut cursor) => self.apply_selection(
          &mut session, &model, &control, &mut cursor, &mut |event| self.observe(event),
        ).await,
        Err(error) => Err(error.into()),
      };
      match prepared {
        Ok(executor::BoundaryResult::Interrupted) => {
          session.finish_run(RunOutcome::Interrupted).map(|_| RunOutcome::Interrupted)
        }
        Ok(_) => executor::compaction::compact(&model, &mut session, &control, |event| self.observe(event)).await,
        Err(error) => Err(error),
      }
    } else {
      executor::run_with_boundary(
        &model,
        &mut session,
        &self.tools,
        &control,
        |event| self.observe(event),
        selection::SelectionBoundary { slot: &self, model: &model },
      )
      .await
    };
    *self.control.lock().unwrap() = None;
    // Observational memory: capture one bounded observation per finished run, when enabled.
    if let Some(app) = self.tools.app_handle() {
      if let Some(memory) = &app.memory {
        let priority = match &result {
          Ok(outcome) => if matches!(outcome, RunOutcome::Completed | RunOutcome::Interrupted) { "normal" } else { "warn" },
          Err(_) => "warn",
        };
        let name = self.descriptor.read().unwrap().name.clone();
        let detail = match &result {
          Ok(outcome) => format!("run finished: {outcome:?}"),
          Err(error) => format!("run failed: {error}"),
        };
        let _ = memory.observe(&self.get_descriptor().id, priority, &format!("{name} — {detail}"));
      }
    }
    if result.is_err() && !session.get_state().is_stable() {
      let _ = session.settle_interrupted();
    }
    let event = match result {
      Ok(outcome) => json!({"type":"operation_finished","outcome":web_outcome(&outcome)}),
      Err(error) => json!({"type":"operation_failed","error":error.to_string()}),
    };
    {
      let mut status = self.status.lock().unwrap();
      *status = self.refresh_status(&session);
      status["last_operation"] = event.clone();
    }
    {
      self.descriptor.write().unwrap().updated_at = crate::session::statistics::Timestamp::now().0;
    }
    if let Err(error) = self.index.save_calls(&self.get_descriptor(), &self.calls) {
      eprintln!("call index: {error}");
    }
    if let Err(error) = self.persist_index() {
      eprintln!("session index: {error}");
    }
    let _ = self.events.send(event);
  }
}
/// The web only needs display deltas and a concise outcome. Opaque replay data stays
/// in session storage, where it can be inspected without flooding every live client.
pub(crate) fn web_event(event: &SessionEvent) -> Option<Value> {
  match event {
    SessionEvent::ModelStream(
      StreamEvent::ReasoningCiphertextDelta { .. }
      | StreamEvent::ReasoningSignatureDelta { .. }
      | StreamEvent::ReasoningReplayItem { .. }
      | StreamEvent::UpstreamCompaction { .. },
    ) => None,
    SessionEvent::Finished(outcome) => Some(json!({"Finished":web_outcome(outcome)})),
    SessionEvent::ResponseInterrupted(_) => Some(json!({"ResponseInterrupted":{}})),
    SessionEvent::ResponseRejected(_) => Some(json!({"ResponseRejected":{}})),
    SessionEvent::UpstreamCompactionCompleted(_) => Some(json!({"UpstreamCompactionCompleted":{}})),
    SessionEvent::CompactionSummary { source_start, source_end, .. } =>
      Some(json!({"CompactionSummary":{"source_start":source_start,"source_end":source_end}})),
    SessionEvent::CompactionSummaryFailed { outcome } =>
      Some(json!({"CompactionSummaryFailed":{"outcome":web_outcome(outcome)}})),
    SessionEvent::CompactionTranslationFailed { .. } =>
      Some(json!({"CompactionTranslationFailed":{}})),
    SessionEvent::ToolStarted(call) => Some(json!({"ToolStarted":{"name":call.name}})),
    SessionEvent::ToolFinished { .. } => Some(json!({"ToolFinished":{}})),
    SessionEvent::MetadataUpdated(_) => Some(json!({"MetadataUpdated":{}})),
    _ => Some(json!(event)),
  }
}
fn web_outcome(outcome: &RunOutcome) -> Value {
  match outcome {
    RunOutcome::StreamFailed(partial) => json!({"StreamFailed":{"reason":partial.reason}}),
    RunOutcome::ModelStopped(response) =>
      json!({"ModelStopped":{"stop_reason":response.stop_reason}}),
    _ => json!(outcome),
  }
}
fn snapshot(session: &Session, stats: Value) -> Value {
  json!({"phase":session.get_state().get_phase(),"state":session.get_state(),
    "active_generation":session.get_active_generation().ok().map(|g|g.id),"metadata":session.get_metadata(),"config":session.get_config(),"queue_head":session.get_queue_head(),"running":false,"standby_preparing":false,
    "context_tokens":context_tokens(session),"stats":stats})
}
/// Running totals behind the stats line under the composer. Settled entries
/// and model calls fold into the memo once; the unsettled tail is summed per
/// read because live records are updated in place.
#[derive(Default)]
struct StatsMemo {
  entries: u64,
  turns: u64,
  calls: u64,
  steps: u64,
  input_tokens: u64,
  output_tokens: u64,
  cached_tokens: u64,
}
impl StatsMemo {
  fn fold(&mut self, call: &ModelCallRecord) {
    if call.purpose == ModelCallPurpose::Conversation {
      self.steps += 1;
    }
    self.input_tokens += call.usage.input_tokens.unwrap_or(0);
    self.output_tokens += call.usage.output_tokens.unwrap_or(0);
    self.cached_tokens += call.usage.cached_input_tokens.unwrap_or(0);
  }
}
/// Folds new settled records into memo, then builds the snapshot stats block
/// from the memo plus the live tail. A shrinking store means it was replaced;
/// everything restarts from scratch.
fn stats_value(
  entries: &ReadList<Entry>,
  calls: &ReadList<ModelCallRecord>,
  memo: &mut StatsMemo,
) -> Value {
  let entry_len = entries.len().unwrap_or(0);
  let call_len = calls.len().unwrap_or(0);
  if entry_len < memo.entries || call_len < memo.calls {
    *memo = StatsMemo::default();
  }
  while memo.entries < entry_len {
    let Ok(Some(entry)) = entries.get(memo.entries) else { break };
    if entry.origin == EntryOrigin::Input {
      memo.turns += 1;
    }
    memo.entries += 1;
  }
  while memo.calls < call_len {
    let Ok(Some(call)) = calls.get(memo.calls) else { break };
    if call.status == ModelCallStatus::Running {
      break;
    }
    memo.fold(&call);
    memo.calls += 1;
  }
  let mut live = StatsMemo::default();
  for position in memo.calls..call_len {
    if let Ok(Some(call)) = calls.get(position) {
      live.fold(&call);
    }
  }
  // The speed of the most recent finished conversation call, in tokens/second.
  let mut rate = None;
  for position in (0..call_len).rev() {
    let Ok(Some(call)) = calls.get(position) else { continue };
    if call.purpose != ModelCallPurpose::Conversation
      || call.status != ModelCallStatus::Completed
    {
      continue;
    }
    let output = call.usage.output_tokens.unwrap_or(0);
    let seconds = match (call.first_event_at, call.finished_at) {
      (Some(first), Some(last)) if last.0 > first.0 => Some((last.0 - first.0) as f64 / 1000.0),
      _ => call.elapsed_ms.map(|ms| ms as f64 / 1000.0),
    };
    if let Some(seconds) = seconds
      && seconds > 0.0
    {
      rate = Some(output as f64 / seconds);
    }
    break;
  }
  let input_tokens = memo.input_tokens + live.input_tokens;
  let cached_tokens = memo.cached_tokens + live.cached_tokens;
  json!({"turns":memo.turns,"steps":memo.steps + live.steps,
    "input_tokens":input_tokens,"output_tokens":memo.output_tokens + live.output_tokens,
    "cached_tokens":cached_tokens,
    "cache_hit":(input_tokens > 0).then(|| cached_tokens as f64 / input_tokens as f64),
    "rate":rate})
}
/// The input size compaction compares with its trigger: the last completed
/// conversation call of the active generation made with the configured model.
fn context_tokens(session: &Session) -> Option<u64> {
  let active = session.get_active_generation().ok()?.id;
  let model = &session.get_config().model;
  let calls = session.get_model_calls();
  for position in (0..calls.len().ok()?).rev() {
    let Some(call) = calls.get(position).ok()? else { continue };
    if call.generation != active {
      break;
    }
    if &call.model == model
      && call.purpose == ModelCallPurpose::Conversation
      && matches!(call.status, ModelCallStatus::Completed)
    {
      return call.last_request_input_tokens;
    }
  }
  None
}
