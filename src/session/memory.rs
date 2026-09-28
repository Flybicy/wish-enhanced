//! Observational memory: observations captured from runs and reflections distilled
//! from them, in a dedicated SQLite database under the data directory. Memory is an
//! appendix to history, never a replacement: compaction may render it, but the ledger
//! keeps every entry forever.

use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;

/// One distilled reflection: durable facts derived from observations.
#[derive(Clone, Debug)]
pub struct Reflection {
  pub id: String,
  pub content: String,
  pub derived_from: Vec<String>,
  pub created_at: i64,
}

/// The memory ledger for one wish installation.
pub struct MemoryStore {
  connection: Mutex<Connection>,
}

fn now_ms() -> i64 {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_millis() as i64)
    .unwrap_or_default()
}

fn short_id() -> String {
  uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
}

impl MemoryStore {
  pub fn open(path: PathBuf) -> Result<Self, rusqlite::Error> {
    if let Some(parent) = path.parent() {
      let _ = std::fs::create_dir_all(parent);
    }
    let connection = Connection::open(path)?;
    connection.execute_batch(
      "PRAGMA journal_mode=WAL;
       CREATE TABLE IF NOT EXISTS observations (
         id TEXT PRIMARY KEY,
         session_id TEXT NOT NULL,
         priority TEXT NOT NULL,
         content TEXT NOT NULL,
         created_at INTEGER NOT NULL
       );
       CREATE TABLE IF NOT EXISTS reflections (
         id TEXT PRIMARY KEY,
         content TEXT NOT NULL,
         derived_from TEXT NOT NULL,
         created_at INTEGER NOT NULL
       );
       CREATE INDEX IF NOT EXISTS observations_session ON observations(session_id, created_at);",
    )?;
    Ok(Self { connection: Mutex::new(connection) })
  }

  /// Captures one observation from a session.
  pub fn observe(&self, session_id: &str, priority: &str, content: &str) -> Result<String, rusqlite::Error> {
    let id = short_id();
    let guard = self.connection.lock().unwrap();
    guard.execute(
      "INSERT INTO observations (id, session_id, priority, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
      rusqlite::params![id, session_id, priority, content, now_ms()],
    )?;
    Ok(id)
  }

  /// Appends one distilled reflection derived from observations.
  pub fn reflect(&self, content: &str, derived_from: &[String]) -> Result<String, rusqlite::Error> {
    let id = short_id();
    let guard = self.connection.lock().unwrap();
    guard.execute(
      "INSERT INTO reflections (id, content, derived_from, created_at) VALUES (?1, ?2, ?3, ?4)",
      rusqlite::params![id, content, serde_json::to_string(derived_from).unwrap_or_default(), now_ms()],
    )?;
    Ok(id)
  }

  /// The most recent reflections, newest first: the durable memory projection.
  pub fn reflections(&self, limit: usize) -> Vec<Reflection> {
    let guard = self.connection.lock().unwrap();
    let Ok(mut statement) = guard.prepare(
      "SELECT id, content, derived_from, created_at FROM reflections ORDER BY created_at DESC, id DESC LIMIT ?1",
    ) else {
      return Vec::new();
    };
    let Ok(rows) = statement.query_map(rusqlite::params![limit as i64], |row| {
      Ok(Reflection {
        id: row.get(0)?,
        content: row.get(1)?,
        derived_from: serde_json::from_str(&row.get::<_, String>(2)?).unwrap_or_default(),
        created_at: row.get(3)?,
      })
    }) else {
      return Vec::new();
    };
    rows.flatten().collect()
  }

  /// Substring search across observations and reflections.
  pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Value>, rusqlite::Error> {
    let guard = self.connection.lock().unwrap();
    let mut results = Vec::new();
    let mut statement = guard.prepare(
      "SELECT 'observation', id, content, created_at FROM observations
         WHERE content LIKE '%' || ?1 || '%'
       UNION ALL
       SELECT 'reflection', id, content, created_at FROM reflections
         WHERE content LIKE '%' || ?1 || '%'
       ORDER BY created_at DESC LIMIT ?2",
    )?;
    let rows = statement.query_map(rusqlite::params![query, limit as i64], |row| {
      Ok(json!({
        "kind": row.get::<_, String>(0)?,
        "id": row.get::<_, String>(1)?,
        "content": row.get::<_, String>(2)?,
        "created_at": row.get::<_, i64>(3)?,
      }))
    })?;
    for row in rows {
      results.push(row?);
    }
    Ok(results)
  }

  /// The rendered memory text for compaction: durable facts, newest first.
  pub fn projection(&self) -> String {
    self
      .reflections(50)
      .into_iter()
      .map(|reflection| reflection.content)
      .collect::<Vec<_>>()
      .join("\n")
  }

  /// Observation and reflection counts for the status surface.
  pub fn counts(&self) -> (usize, usize) {
    let guard = self.connection.lock().unwrap();
    let observations = guard
      .query_row("SELECT COUNT(*) FROM observations", [], |row| row.get::<_, i64>(0))
      .unwrap_or_default() as usize;
    let reflections = guard
      .query_row("SELECT COUNT(*) FROM reflections", [], |row| row.get::<_, i64>(0))
      .unwrap_or_default() as usize;
    (observations, reflections)
  }
}

/// One memory recall tool over a shared store.
pub struct MemoryRecallTool {
  pub store: std::sync::Arc<MemoryStore>,
}

impl MemoryRecallTool {
  pub fn get_specification(&self) -> crate::protocol::Tool {
    crate::protocol::Tool {
      name: "memory_recall".into(),
      description: "Search long-term memory: past observations and distilled reflections from         all sessions. Returns matching entries with their ids."
        .into(),
      input_schema: serde_json::json!({
        "type": "object",
        "properties": {
          "query": {"type": "string", "description": "Words to look for in memory"},
          "limit": {"type": "integer", "minimum": 1, "maximum": 50,
            "description": "How many entries to return (default 10)"}
        },
        "required": ["query"]
      }),
    }
  }

  pub fn get_status(&self) -> Value {
    let (observations, reflections) = self.store.counts();
    serde_json::json!({"observations": observations, "reflections": reflections})
  }
}

impl crate::executor::tool::ToolExecutor for MemoryRecallTool {
  async fn execute(
    &self,
    call: &crate::executor::tool::ToolCall,
    _control: &crate::executor::ExecutionControl,
  ) -> crate::executor::tool::ToolOutcome {
    use crate::executor::tool::ToolOutcome;
    let Some(query) = call.arguments.get("query").and_then(Value::as_str) else {
      return ToolOutcome::Failed("query is required".into());
    };
    let limit = call.arguments.get("limit").and_then(Value::as_u64).unwrap_or(10).min(50) as usize;
    match self.store.search(query, limit) {
      Ok(entries) => ToolOutcome::Success(json!({"entries": entries, "total": entries.len()})),
      Err(error) => ToolOutcome::Failed(format!("memory search failed: {error}")),
    }
  }
}