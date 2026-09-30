//! MCP (Model Context Protocol) stdio client: one child process per server.
//! JSON-RPC 2.0 with newline-delimited frames over the child's stdin and stdout.
//! A mid-call disconnect yields `ToolOutcome::Unknown`: the external effect is unknown.

use crate::executor::tool::ToolOutcome;
use crate::protocol::Tool;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};
use tokio::time::timeout;

/// How to launch one MCP server and how long its calls may take.
#[derive(Clone, Debug)]
pub struct McpServerSpec {
  pub command: String,
  pub args: Vec<String>,
  pub env: BTreeMap<String, String>,
  pub call_timeout: Duration,
}

type PendingMap = Arc<Mutex<BTreeMap<i64, oneshot::Sender<Result<Value, String>>>>>;

/// A live connection to one MCP server process.
struct Connection {
  child: Child,
  /// Shared with the reader task so server-initiated requests can be declined.
  writer: Arc<Mutex<tokio::process::ChildStdin>>,
  pending: PendingMap,
  /// The protocol version the server answered with.
  protocol_version: String,
}

/// All configured MCP servers, keyed by server ID.
pub struct McpRegistry {
  connections: Mutex<BTreeMap<String, Connection>>,
  next_id: AtomicI64,
}

impl McpRegistry {
  pub fn new() -> Self {
    Self { connections: Mutex::new(BTreeMap::new()), next_id: AtomicI64::new(1) }
  }

  /// Spawns the server process and completes the initialize handshake. A server that
  /// fails to start or answer initialize is not registered; retry with a corrected spec.
  pub async fn start(&self, id: &str, spec: &McpServerSpec) -> Result<(), String> {
    let mut command = Command::new(&spec.command);
    command.args(&spec.args);
    // Bundled tools resolve next to the executable; the server env can still override PATH.
    if let Some(path) = super::bundled::child_path() {
      command.env("PATH", path);
    }
    let mut child = command
      .envs(&spec.env)
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::null())
      .kill_on_drop(true)
      .spawn()
      .map_err(|error| format!("mcp server {id} spawn failed: {error}"))?;
    let raw_stdin =
      child.stdin.take().ok_or_else(|| format!("mcp server {id} has no stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| format!("mcp server {id} has no stdout"))?;
    let pending: PendingMap = Arc::new(Mutex::new(BTreeMap::new()));
    let writer = Arc::new(Mutex::new(raw_stdin));
    let (ready_tx, ready_rx) = oneshot::channel::<Result<(), String>>();
    spawn_reader(stdout, pending.clone(), writer.clone(), ready_tx);
    ready_rx.await.map_err(|_| format!("mcp server {id} reader task died"))??;
    let connection = Connection {
      child,
      writer,
      pending,
      protocol_version: String::new(),
    };
    self.connections.lock().await.insert(id.to_owned(), connection);
    let response = self
      .request(
        id,
        "initialize",
        json!({
          "protocolVersion": "2025-06-18",
          "capabilities": {},
          "clientInfo": {"name": "wish", "version": env!("CARGO_PKG_VERSION")}
        }),
        spec.call_timeout,
      )
      .await
      .map_err(|error| format!("mcp server {id} initialize failed: {error}"))?;
    let version = response
      .get("protocolVersion")
      .and_then(Value::as_str)
      .unwrap_or("2025-06-18")
      .to_owned();
    if let Some(entry) = self.connections.lock().await.get_mut(id) {
      entry.protocol_version = version;
    }
    self.notify(id, "notifications/initialized", json!({})).await?;
    Ok(())
  }

  /// Tools this server declares, mapped onto the engine's tool schema verbatim.
  pub async fn tools_of(&self, id: &str, spec: &McpServerSpec) -> Result<Vec<Tool>, String> {
    let response = self.request(id, "tools/list", json!({}), spec.call_timeout).await?;
    let mut tools = Vec::new();
    if let Some(items) = response.get("tools").and_then(Value::as_array) {
      for item in items {
        let Some(name) = item.get("name").and_then(Value::as_str) else { continue };
        tools.push(Tool {
          name: name.to_owned(),
          description: item.get("description").and_then(Value::as_str).unwrap_or("").to_owned(),
          input_schema: item.get("inputSchema").cloned().unwrap_or_else(|| json!({})),
        });
      }
    }
    Ok(tools)
  }

  /// Calls one tool. Cancellation notifies the server; a dead connection reports Unknown.
  pub async fn call_tool(
    &self,
    id: &str,
    tool: &str,
    arguments: Value,
    spec: &McpServerSpec,
    control: &crate::executor::ExecutionControl,
  ) -> ToolOutcome {
    let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
    let payload = json!({
      "jsonrpc": "2.0",
      "id": request_id,
      "method": "tools/call",
      "params": {"name": tool, "arguments": arguments}
    });
    let send = async {
      let connections = self.connections.lock().await;
      let Some(connection) = connections.get(id) else { return None };
      let mut writer = connection.writer.lock().await;
      writer.write_all(format!("{payload}\n").as_bytes()).await.ok()?;
      writer.flush().await.ok()?;
      Some(())
    };
    if timeout(Duration::from_secs(5), send).await.ok().flatten().is_none() {
      return ToolOutcome::Unknown(format!("mcp server {id} is not accepting calls"));
    }
    // Park the oneshot before awaiting cancellation so the reader can complete us either way.
    let (tx, rx) = oneshot::channel();
    self.register(request_id, id, tx).await;
    let cancellation = async {
      while !control.is_cancelled() {
        tokio::time::sleep(Duration::from_millis(100)).await;
      }
    };
    let result = tokio::select! {
      response = timeout(spec.call_timeout, rx) => match response {
        Ok(Ok(Ok(value))) => Ok(value),
        Ok(Ok(Err(message))) => Err(message),
        Ok(Err(_)) => Err(format!("mcp server {id} dropped the response")),
        Err(_) => Err(format!("mcp server {id} timed out after {:?}", spec.call_timeout)),
      },
      () = cancellation => Err("__cancelled__".to_owned()),
    };
    match result {
      Ok(value) => decode_result(value),
      Err(message) if message == "__cancelled__" => {
        self.notify(id, "notifications/cancelled", json!({"requestId": request_id})).await.ok();
        self.forget(request_id).await;
        ToolOutcome::Cancelled
      }
      Err(message) => ToolOutcome::Failed(message),
    }
  }

  /// True while the server process answers; a server that exited reports false.
  pub async fn is_running(&self, id: &str) -> bool {
    match self.connections.lock().await.get_mut(id) {
      Some(connection) => connection.child.try_wait().ok().map_or(true, |state| state.is_none()),
      None => false,
    }
  }

  /// Stops one server: cancels pending calls as Unknown and kills the child.
  pub async fn stop(&self, id: &str) {
    if let Some(mut connection) = self.connections.lock().await.remove(id) {
      let taken: BTreeMap<i64, oneshot::Sender<Result<Value, String>>> =
        std::mem::take(&mut *connection.pending.lock().await);
      for (_, reply) in taken {
        let _ = reply.send(Err(format!("mcp server {id} was stopped")));
      }
      let _ = connection.child.kill().await;
    }
  }

  /// Stops every server; called during application shutdown.
  pub async fn shutdown(&self) {
    let ids: Vec<String> = self.connections.lock().await.keys().cloned().collect();
    for id in ids {
      self.stop(&id).await;
    }
  }

  async fn request(&self, id: &str, method: &str, params: Value, deadline: Duration) -> Result<Value, String> {
    let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
    let payload = json!({"jsonrpc":"2.0","id":request_id,"method":method,"params":params});
    {
      let connections = self.connections.lock().await;
      let connection =
        connections.get(id).ok_or_else(|| format!("mcp server {id} is not running"))?;
      let writer = connection.writer.clone();
      drop(connections);
      let mut guard = writer.lock().await;
      guard.write_all(format!("{payload}\n").as_bytes()).await
        .map_err(|error| format!("mcp server {id} write failed: {error}"))?;
      guard.flush().await
        .map_err(|error| format!("mcp server {id} write failed: {error}"))?;
    }
    let (tx, rx) = oneshot::channel();
    self.register(request_id, id, tx).await;
    match timeout(deadline, rx).await {
      Ok(Ok(Ok(value))) => Ok(value),
      Ok(Ok(Err(message))) => Err(message),
      Ok(Err(_)) => Err(format!("mcp server {id} dropped the response")),
      Err(_) => {
        self.forget(request_id).await;
        Err(format!("mcp server {id} timed out after {deadline:?}"))
      }
    }
  }

  async fn notify(&self, id: &str, method: &str, params: Value) -> Result<(), String> {
    let payload = json!({"jsonrpc":"2.0","method":method,"params":params});
    let writer = {
      let connections = self.connections.lock().await;
      let connection =
        connections.get(id).ok_or_else(|| format!("mcp server {id} is not running"))?;
      connection.writer.clone()
    };
    let mut guard = writer.lock().await;
    guard.write_all(format!("{payload}\n").as_bytes()).await
      .map_err(|error| format!("mcp server {id} write failed: {error}"))?;
    guard.flush().await
      .map_err(|error| format!("mcp server {id} write failed: {error}"))?;
    Ok(())
  }

  async fn register(&self, request_id: i64, server: &str, tx: oneshot::Sender<Result<Value, String>>) {
    if let Some(connection) = self.connections.lock().await.get_mut(server) {
      connection.pending.lock().await.insert(request_id, tx);
    }
  }

  async fn forget(&self, request_id: i64) {
    for connection in self.connections.lock().await.values() {
      connection.pending.lock().await.remove(&request_id);
    }
  }
}

fn spawn_reader(
  stdout: tokio::process::ChildStdout,
  pending: PendingMap,
  writer: Arc<Mutex<tokio::process::ChildStdin>>,
  ready: oneshot::Sender<Result<(), String>>,
) {
  tokio::spawn(async move {
    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();
    ready.send(Ok(())).ok();
    loop {
      let line = match lines.next_line().await {
        Ok(Some(line)) => line,
        Ok(None) | Err(_) => break,
      };
      let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
      let has_payload = message.get("result").is_some() || message.get("error").is_some();
      if let Some(id) = message.get("id").and_then(Value::as_i64) {
        if has_payload {
          let reply = pending.lock().await.remove(&id);
          if let Some(reply) = reply {
            if let Some(error) = message.get("error") {
              let text = error.get("message").and_then(Value::as_str).unwrap_or("mcp error");
              let _ = reply.send(Err(text.to_owned()));
            } else {
              let _ = reply.send(Ok(message["result"].clone()));
            }
          }
        } else if id >= 0 {
          let decline = json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": "not supported by wish"}
          });
          let mut guard = writer.lock().await;
          let _ = guard.write_all(format!("{decline}\n").as_bytes()).await;
          let _ = guard.flush().await;
        }
      }
    }
  });
}

/// Maps an MCP tools/call result onto the engine's outcome vocabulary. Text content is
/// joined; results over 256 KiB are truncated; `isError` failures become `Failed`.
fn decode_result(value: Value) -> ToolOutcome {
  let is_error = value.get("isError").and_then(Value::as_bool).unwrap_or(false);
  let mut text = String::new();
  if let Some(content) = value.get("content").and_then(Value::as_array) {
    for block in content {
      match block.get("type").and_then(Value::as_str) {
        Some("text") => {
          if let Some(item) = block.get("text").and_then(Value::as_str) {
            if !text.is_empty() {
              text.push('\n');
            }
            text.push_str(item);
          }
        }
        Some("image") | Some("audio") | Some("resource") => {
          text.push_str("\n[binary content omitted]");
        }
        _ => {}
      }
    }
  }
  if text.len() > 256 * 1024 {
    text.truncate(256 * 1024);
    text.push_str("\n... [truncated at 256 KiB]");
  }
  if is_error {
    ToolOutcome::Failed(if text.is_empty() { "mcp tool reported an error".into() } else { text })
  } else {
    ToolOutcome::Success(json!({"content": text}))
  }
}

/// One MCP server reached over streamable HTTP: every request is a POST carrying the
/// JSON-RPC message; replies arrive as one JSON body or as an SSE stream of messages.
pub struct HttpMcpServer {
  pub endpoint: String,
  pub headers: BTreeMap<String, String>,
  pub call_timeout: Duration,
  next_id: AtomicI64,
}

impl HttpMcpServer {
  pub fn new(endpoint: String, headers: BTreeMap<String, String>, call_timeout: Duration) -> Self {
    Self { endpoint, headers, call_timeout, next_id: AtomicI64::new(1) }
  }

  fn client(&self) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
      .timeout(self.call_timeout)
      .build()
      .map_err(|error| format!("http client error: {error}"))
  }

  async fn post(&self, payload: &Value) -> Result<Value, String> {
    let client = self.client()?;
    let mut request = client
      .post(&self.endpoint)
      .header("Accept", "application/json, text/event-stream")
      .header("MCP-Protocol-Version", "2025-06-18")
      .json(payload);
    for (key, value) in &self.headers {
      request = request.header(key, value);
    }
    let response = request.send().await
      .map_err(|error| format!("mcp http request failed: {error}"))?;
    if !response.status().is_success() {
      return Err(format!("mcp http returned HTTP {}", response.status()));
    }
    let content_type = response
      .headers()
      .get(reqwest::header::CONTENT_TYPE)
      .and_then(|value| value.to_str().ok())
      .unwrap_or("")
      .to_owned();
    let body = response.text().await
      .map_err(|error| format!("mcp http body read failed: {error}"))?;
    if content_type.contains("text/event-stream") {
      for line in body.lines() {
        if let Some(data) = line.strip_prefix("data:") {
          if let Ok(message) = serde_json::from_str::<Value>(data.trim()) {
            if message.get("result").is_some() || message.get("error").is_some() {
              return decode_rpc(message);
            }
          }
        }
      }
      return Err("mcp http stream carried no response".into());
    }
    let message: Value = serde_json::from_str(&body)
      .map_err(|error| format!("mcp http body parse failed: {error}"))?;
    decode_rpc(message)
  }

  /// Performs initialize and tools/list in one session-less exchange.
  pub async fn tools_of(&self) -> Result<Vec<Tool>, String> {
    self.post(&json!({
      "jsonrpc": "2.0",
      "id": self.next_id.fetch_add(1, Ordering::Relaxed),
      "method": "initialize",
      "params": {
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": {"name": "wish", "version": env!("CARGO_PKG_VERSION")}
      }
    })).await?;
    self.post(&json!({
      "jsonrpc": "2.0",
      "id": self.next_id.fetch_add(1, Ordering::Relaxed),
      "method": "tools/list",
      "params": {}
    })).await.map(|response| {
      let mut tools = Vec::new();
      if let Some(items) = response.get("tools").and_then(Value::as_array) {
        for item in items {
          let Some(name) = item.get("name").and_then(Value::as_str) else { continue };
          tools.push(Tool {
            name: name.to_owned(),
            description: item.get("description").and_then(Value::as_str).unwrap_or("").to_owned(),
            input_schema: item.get("inputSchema").cloned().unwrap_or_else(|| json!({})),
          });
        }
      }
      tools
    })
  }

  /// Calls one tool over HTTP; cancellation abandons the POST.
  pub async fn call_tool(
    &self,
    tool: &str,
    arguments: Value,
    control: &crate::executor::ExecutionControl,
  ) -> ToolOutcome {
    let payload = json!({
      "jsonrpc": "2.0",
      "id": self.next_id.fetch_add(1, Ordering::Relaxed),
      "method": "tools/call",
      "params": {"name": tool, "arguments": arguments}
    });
    let call = self.post(&payload);
    tokio::pin!(call);
    tokio::select! {
      result = &mut call => match result {
        Ok(value) => decode_result(value),
        Err(message) => ToolOutcome::Failed(message),
      },
      () = async {
        while !control.is_cancelled() {
          tokio::time::sleep(Duration::from_millis(100)).await;
        }
      } => ToolOutcome::Cancelled,
    }
  }
}

/// Pulls the result (or error) out of one JSON-RPC reply envelope.
fn decode_rpc(message: Value) -> Result<Value, String> {
  if let Some(error) = message.get("error") {
    let text = error.get("message").and_then(Value::as_str).unwrap_or("mcp error");
    Err(text.to_owned())
  } else {
    Ok(message.get("result").cloned().unwrap_or(Value::Null))
  }
}