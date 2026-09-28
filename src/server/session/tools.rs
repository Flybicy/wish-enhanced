use crate::server::{app::App, error::blocking};
use crate::{
  executor::{
    ExecutionControl,
    tool::{ToolCall, ToolExecutor, ToolOutcome},
  },
  protocol::{ContentBlock, Message},
  session::{Session, SessionHandle},
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