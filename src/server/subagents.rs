//! In-process subagent delegation: a subagent is a full session with its own history,
//! executed through the normal scheduler, bounded by a concurrency pool. Completion is
//! delivered back to the parent session as a Developer message, exactly like the shell
//! tool's background notifications.
#![allow(dead_code)]

use crate::server::app::App;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Records one delegated subagent from spawn to completion.
#[derive(Clone, Debug)]
pub struct AgentRecord {
  pub id: String,
  pub parent_session: String,
  pub agent_type: String,
  pub prompt: String,
  pub description: String,
  pub status: String,
  /// Delegation depth: 1 under a root session, one more under a subagent.
  pub depth: usize,
}

/// Manages the pool and the registry of subagents.
pub struct SubagentManager {
  pub permits: Arc<Semaphore>,
  records: tokio::sync::Mutex<BTreeMap<String, AgentRecord>>,
  counter: AtomicU64,
  pub max_depth: usize,
}

impl SubagentManager {
  pub fn new(max_concurrent: usize, max_depth: usize) -> Self {
    Self {
      permits: Arc::new(Semaphore::new(max_concurrent.max(1))),
      records: tokio::sync::Mutex::new(BTreeMap::new()),
      counter: AtomicU64::new(1),
      max_depth,
    }
  }

  pub async fn spawn(
    &self,
    parent_session: String,
    agent_type: String,
    prompt: String,
    description: String,
    depth: usize,
  ) -> String {
    let id = format!("agent-{}", self.counter.fetch_add(1, Ordering::Relaxed));
    let record = AgentRecord {
      id: id.clone(),
      parent_session,
      agent_type,
      prompt,
      description,
      status: "queued".into(),
      depth,
    };
    self.records.lock().await.insert(id.clone(), record);
    id
  }

  pub async fn get(&self, id: &str) -> Option<AgentRecord> {
    self.records.lock().await.get(id).cloned()
  }

  pub async fn set_depth(&self, id: &str, depth: usize) {
    if let Some(record) = self.records.lock().await.get_mut(id) {
      record.depth = depth;
    }
  }
  pub async fn update_status(&self, id: &str, status: &str) {
    if let Some(record) = self.records.lock().await.get_mut(id) {
      record.status = status.to_owned();
    }
  }

  pub async fn list_for_parent(&self, parent: &str) -> Vec<AgentRecord> {
    self
      .records
      .lock()
      .await
      .values()
      .filter(|record| record.parent_session == parent)
      .cloned()
      .collect()
  }
}

/// Builds the Agent tool specification shown to the model.
pub fn agent_specification(_manager: &SubagentManager) -> crate::protocol::Tool {
  crate::protocol::Tool {
    name: "subagent".into(),
    description: "Delegate a task to an autonomous subagent with its own session, working       through the same tools. Returns immediately with an agent id; the result is delivered       to this conversation when the subagent finishes.".into(),
    input_schema: json!({
      "type": "object",
      "properties": {
        "prompt": {"type": "string", "description": "The complete, self-contained task"},
        "description": {"type": "string", "description": "A 3-5 word label for the task"},
        "agent_type": {"type": "string",
          "description": "general-purpose (default) or explore (read-only tools)"}
      },
      "required": ["prompt"]
    }),
  }
}

/// Builds the result-reading tool specification.
pub fn result_specification() -> crate::protocol::Tool {
  crate::protocol::Tool {
    name: "subagent_result".into(),
    description: "Read the status and transcript tail of one subagent by id.".into(),
    input_schema: json!({
      "type": "object",
      "properties": {
        "agent_id": {"type": "string", "description": "The id returned by subagent"}
      },
      "required": ["agent_id"]
    }),
  }
}

pub fn get_specifications(manager: &SubagentManager) -> Vec<crate::protocol::Tool> {
  vec![agent_specification(manager), result_specification()]
}

/// Spawns the subagent session and schedules its run; completion notifies the parent.
/// Attenuation: a subagent works inside the parent's working directory, without
/// `ask_user` (nobody is watching to answer), without MCP, skills-over-bridge,
/// peer sessions or web search, and - for explore agents - without a shell at
/// all, under the read-only permission mode. Delegation depth is bounded by the
/// manager's `max_depth`.
pub async fn launch(
  app: Arc<App>,
  parent_id: String,
  agent_id: String,
  agent_type: String,
  prompt: String,
  description: String,
) -> Result<Value, String> {
  let manager = app.subagents.clone();
  manager.update_status(&agent_id, "starting").await;
  let parent = app
    .get_session(&parent_id)
    .await
    .map_err(|error| format!("parent session is unavailable: {error}"))?;
  let descriptor = parent.get_descriptor();
  // Depth rides the session's metadata: a root session is 0, a subagent one more.
  let (parent_depth, cwd) = {
    let session = parent.session.lock().await;
    let depth = session
      .get_metadata()["subagent"]["depth"]
      .as_u64()
      .unwrap_or(0) as usize;
    (depth, descriptor.cwd.clone())
  };
  let depth = parent_depth + 1;
  manager.set_depth(&agent_id, depth).await;
  if parent_depth >= manager.max_depth {
    return Err(format!(
      "delegation depth limit reached ({}); run the task in this session instead",
      manager.max_depth
    ));
  }
  let provider_id = {
    let configuration = app.config_file.lock().await;
    if configuration.config.defaults.provider.is_empty() {
      "openai".to_string()
    } else {
      configuration.config.defaults.provider.clone()
    }
  };
  let model = {
    let configuration = app.config_file.lock().await;
    if configuration.config.defaults.model.is_empty() { String::new() } else { configuration.config.defaults.model.clone() }
  };
  let mut config = crate::session::SessionConfig::new(model);
  config.tools = Vec::new();
  // Explore agents only read; general subagents act, but none may ask the user.
  let read_only = agent_type == "explore";
  config.permission = if read_only {
    crate::session::PermissionMode::ReadOnly
  } else {
    crate::session::PermissionMode::Operate
  };
  let slot = app
    .create_session(crate::server::session::CreateSession {
      initial_messages: Vec::new(),
      initial_origins: Vec::new(),
      name: format!("subagent {agent_id}: {description}"),
      provider: provider_id.clone(),
      config,
      cwd,
      metadata: json!({"subagent": {"id": agent_id, "parent": parent_id, "depth": depth}}),
      created_by: None,
      folder: None,
      tools: crate::server::session::ToolChanges {
        shell: Some(!read_only),
        ask_user: Some(false),
        mcp: Some(false),
        web_search: Some(false),
        skills: Some(false),
        sessions: Some(false),
      },
    })
    .await
    .map_err(|error| format!("subagent session create failed: {error}"))?;
  let session_id = slot.get_descriptor().id.clone();
  manager.update_status(&agent_id, "running").await;
  // Inject the task as the first user input, then schedule the run.
  let message = crate::protocol::Message::User {
    metadata: json!({"source": "subagent", "agent_id": agent_id}),
    content: vec![crate::protocol::ContentBlock::Text { text: prompt }],
  };
  let entry = slot
    .enqueue(message)
    .await
    .map_err(|error| format!("subagent enqueue failed: {error}"))?;
  let _ = app.events.send(json!({"type": "session_changed", "id": session_id}));
  slot.schedule(&app);
  // Completion watcher: when the session idles with no queued input, deliver to the parent.
  let watcher_app = app.clone();
  let watcher_slot = slot.clone();
  let watcher_parent = parent_id.clone();
  let watcher_agent = agent_id.clone();
  app.lifecycle.tasks.spawn(async move {
    loop {
      tokio::time::sleep(std::time::Duration::from_secs(2)).await;
      let session = watcher_slot.session.lock().await;
      if !watcher_slot.get_status().status.running
        && session.get_state().is_stable()
      {
        // Idle. Deliver a completion notice with the descriptor summary; the full
        // transcript lives in the subagent session and stays browsable.
        drop(session);
        let descriptor = watcher_slot.get_descriptor();
        let summary = format!(
          "session: {}
name: {}
messages processed; open the session to read the transcript.",
          descriptor.id, descriptor.name
        );
        manager.update_status(&watcher_agent, "completed").await;
        deliver_to_parent(&watcher_app, &watcher_parent, &watcher_agent, &summary).await;
        break;
      }
    }
  });
  Ok(json!({"agent_id": agent_id, "session_id": session_id, "entry": entry, "status": "background", "depth": depth}))
}

/// Delivers a finished subagent's summary to its parent session as a Developer message.
async fn deliver_to_parent(app: &Arc<App>, parent_id: &str, agent_id: &str, summary: &str) {
  let Ok(parent) = app.get_session(parent_id).await else { return };
  let message = crate::protocol::Message::Developer {
    metadata: json!({"source": "subagent_finished", "agent_id": agent_id}),
    fixed: Some(false),
    content: vec![crate::protocol::ContentBlock::Text {
      text: format!("Subagent {agent_id} finished.\n{summary}"),
    }],
  };
  let _ = parent.enqueue(message).await;
  let _ = app.events.send(json!({"type": "session_changed", "id": parent_id}));
  parent.schedule(app);
}