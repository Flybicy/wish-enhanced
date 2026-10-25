//! What a session's status reports, kept current from the engine's events, and the events a web
//! client is sent: display deltas and short outcomes, never replay data.
use super::{Descriptor, SessionSlot};
use crate::{
  protocol::StreamEvent,
  session::{
    Entry, EntryId, EntryOrigin, GenerationId, PermissionMode, RunOutcome, Session, SessionConfig,
    SessionEvent, SessionPhase, SessionState,
    statistics::{ModelCallPurpose, ModelCallRecord, ModelCallStatus},
  },
  storage::ReadList,
};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// The engine's part of the status, as the session last reported it.
#[derive(Clone, Serialize)]
pub struct SessionStatus {
  pub phase: SessionPhase,
  /// Absent while an operation runs, when the state changes under it.
  #[serde(skip_serializing_if = "Option::is_none")]
  state: Option<SessionState>,
  active_generation: Option<GenerationId>,
  metadata: Value,
  pub config: SessionConfig,
  /// The live permission mode; it gates side-effect tools even mid-run.
  pub permission: PermissionMode,
  queue_head: u64,
  pub running: bool,
  standby_preparing: bool,
  context_tokens: Option<u64>,
  /// How the last operation ended, until the status is next read from the session.
  #[serde(skip_serializing_if = "Option::is_none")]
  last_operation: Option<Value>,
}
impl SessionStatus {
  /// The status of a session no operation runs on.
  pub fn read(session: &Session) -> Self {
    Self {
      phase: session.get_state().get_phase(),
      state: Some(session.get_state().clone()),
      active_generation: session.get_active_generation().ok().map(|generation| generation.id),
      metadata: session.get_metadata().clone(),
      config: session.get_config().clone(),
      permission: session.get_config().permission,
      queue_head: session.get_queue_head(),
      running: false,
      standby_preparing: false,
      context_tokens: context_tokens(session),
      last_operation: None,
    }
  }
  /// Marks an operation begun.
  pub fn begin_operation(&mut self) {
    self.running = true;
    self.state = None;
  }
  /// The status once an operation ended, reporting how.
  pub fn after_operation(session: &Session, ending: Value) -> Self {
    Self { last_operation: Some(ending), ..Self::read(session) }
  }
}

/// The status as the API shows it: with the queue's length, the open questions, and a pending
/// model selection shown as made.
#[derive(Serialize)]
pub struct StatusView {
  #[serde(flatten)]
  pub status: SessionStatus,
  pub queue_count: u64,
  pending_questions: Vec<Value>,
  #[serde(skip_serializing_if = "std::ops::Not::not")]
  selection_pending: bool,
  /// Ask-mode approvals awaiting a user decision, with a redacted summary.
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pending_approvals: Vec<Value>,
  /// Running totals behind the stats line under the composer.
  pub stats: Value,
}

impl SessionSlot {
  /// Reads the status afresh from the session, which no operation holds.
  pub fn refresh_status(&self, session: &Session) {
    *self.status.lock().unwrap() = SessionStatus::read(session);
  }
  pub fn get_status(&self) -> StatusView {
    self.view_status(&self.get_descriptor())
  }
  /// The status as the API shows it for this descriptor.
  pub(super) fn view_status(&self, descriptor: &Descriptor) -> StatusView {
    let mut status = self.status.lock().unwrap().clone();
    let queued = self.reader.get_message_queue().len().unwrap_or(0);
    let queue_count = queued.saturating_sub(status.queue_head);
    let pending_questions = self.tools.ask_user.snapshot();
    let pending_approvals = self.list_pending_approvals();
    let stats = {
      let entries = self.reader.get_entries();
      let calls = self.reader.get_model_calls();
      let mut memo = self.stats.lock().unwrap();
      stats_value(&entries, &calls, &mut memo)
    };
    let selection_pending = match &descriptor.pending_selection {
      Some(pending) => {
        status.config = pending.config.clone();
        true
      }
      None => false,
    };
    StatusView { status, queue_count, pending_questions, selection_pending, pending_approvals, stats }
  }
  /// Follows an engine event: the status, the live preview, and the event's web form to live
  /// clients. A phase change is saved to the index.
  pub fn observe(&self, event: &SessionEvent) {
    let phase_changed = {
      let mut status = self.status.lock().unwrap();
      match event {
        SessionEvent::InputsConsumed { queue_end, .. } => status.queue_head = *queue_end,
        SessionEvent::StateChanged { to, .. } => status.phase = *to,
        SessionEvent::CompactionSummaryStarted { .. } => status.standby_preparing = true,
        SessionEvent::CompactionSummary { .. }
        | SessionEvent::CompactionSummaryFailed { .. }
        | SessionEvent::ContextCompacted { .. } => status.standby_preparing = false,
        _ => {}
      }
      matches!(event, SessionEvent::StateChanged { .. })
    };
    if phase_changed && let Err(error) = self.persist_index() {
      eprintln!("session index: {error}");
    }
    let mut live = self.live.lock().unwrap();
    live.observe(event);
    if let Some(event) = web_event(event) {
      let _ =
        self.events.send(json!({"type":"session_event","event":event,"revision":live.revision}));
    }
  }
  /// Notes that a selection applied at a boundary: its config is the session's now, and a standby
  /// prepared for the old model is gone.
  pub(super) fn selection_applied(&self, config: &SessionConfig) {
    let mut status = self.status.lock().unwrap();
    status.config = config.clone();
    status.standby_preparing = false;
  }
  pub fn subscribe_live(&self) -> (broadcast::Receiver<Value>, Value) {
    // Subscription and snapshot share the publication lock: no gap or duplicate deltas.
    let live = self.live.lock().unwrap();
    (self.events.subscribe(), live.snapshot(self.describe()))
  }
}

/// The web only needs display deltas and a concise outcome. Opaque replay data stays
/// in session storage, where it can be inspected without flooding every live client.
pub fn web_event(event: &SessionEvent) -> Option<Value> {
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
    SessionEvent::CompactionSummary { source_start, source_end, .. } => {
      Some(json!({"CompactionSummary":{"source_start":source_start,"source_end":source_end}}))
    }
    SessionEvent::CompactionSummaryFailed { outcome } => {
      Some(json!({"CompactionSummaryFailed":{"outcome":web_outcome(outcome)}}))
    }
    SessionEvent::CompactionTranslationFailed { .. } => {
      Some(json!({"CompactionTranslationFailed":{}}))
    }
    SessionEvent::ToolStarted(call) => Some(json!({"ToolStarted":{"name":call.name}})),
    SessionEvent::ToolFinished { .. } => Some(json!({"ToolFinished":{}})),
    SessionEvent::MetadataUpdated(_) => Some(json!({"MetadataUpdated":{}})),
    _ => Some(json!(event)),
  }
}
/// An outcome without the partial or full response it may carry.
pub(super) fn web_outcome(outcome: &RunOutcome) -> Value {
  match outcome {
    RunOutcome::StreamFailed(partial) => json!({"StreamFailed":{"reason":partial.reason}}),
    RunOutcome::ModelStopped(response) => {
      json!({"ModelStopped":{"stop_reason":response.stop_reason}})
    }
    _ => json!(outcome),
  }
}
/// Running totals behind the stats line under the composer. Settled entries
/// and model calls fold into the memo once; the unsettled tail is summed per
/// read because live records are updated in place.
#[derive(Default)]
pub(super) struct StatsMemo {
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
/// Folds new settled records into the memo, then builds the status stats block
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
  let calls = session.reader().get_model_calls();
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
