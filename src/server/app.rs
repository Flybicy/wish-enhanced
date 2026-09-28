use crate::server::{
  config::Config,
  error::{ApiError, blocking},
  provider::Provider,
  session::{CreateSession, Descriptor, SessionSlot},
};
use crate::{
  session::Session,
  storage::{Storage, StorageOptions},
};
use std::{
  collections::BTreeMap,
  path::PathBuf,
  sync::{Arc, Mutex, RwLock},
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub struct App {
  pub storage: Storage,
  pub index: Arc<crate::server::management::ManagementStore>,
  pub configuration: AsyncMutex<crate::server::configuration::Configuration>,
  pub codex_login: crate::server::codex_login::LoginManager,
  pub events: tokio::sync::broadcast::Sender<serde_json::Value>,
  pub started: std::time::Instant,
  pub providers: RwLock<BTreeMap<String, Arc<Provider>>>,
  /// Shared with every session's shell tool; a configuration save replaces its value.
  pub shell: Arc<RwLock<crate::tool::shell::ShellCommand>>,
  pub sessions: AsyncMutex<BTreeMap<String, Arc<SessionSlot>>>,
  pub data_dir: PathBuf,
  /// The directory holding the bundled web interface, when one is installed.
  pub web_dir: Option<PathBuf>,
  pub token: Option<String>,
  pub stop: CancellationToken,
  pub tasks: TaskTracker,
  /// Serializes operation registration against shutdown, never held across awaits.
  pub closing: Mutex<bool>,
  /// Shared web access tools; present when the configuration enables them.
  pub web: Option<Arc<crate::tool::web::WebTool>>,
  /// MCP server registry; servers start from the config and stop at shutdown.
  pub mcp: Arc<crate::tool::mcp::McpRegistry>,
  /// The launch specifications of every configured MCP server, by ID.
  pub mcp_specs: Arc<tokio::sync::Mutex<BTreeMap<String, crate::tool::mcp::McpServerSpec>>>,
  /// Tool schemas captured from each MCP server at startup, namespaced per server.
  pub mcp_tools: std::sync::Mutex<BTreeMap<String, Vec<crate::protocol::Tool>>>,
  /// Remote MCP servers over streamable HTTP, by ID; calls go straight to HTTP.
  pub mcp_http: std::sync::Mutex<BTreeMap<String, Arc<crate::tool::mcp::HttpMcpServer>>>,
  /// The observational memory ledger, present when memory is enabled.
  pub memory: Option<Arc<crate::session::memory::MemoryStore>>,
  /// Subagent delegation pool and registry.
  pub subagents: Arc<crate::server::subagents::SubagentManager>,
  /// Whether the configuration enables subagent delegation.
  pub subagents_enabled: bool,
}
impl App {
  pub async fn open(config: &Config, config_path: PathBuf) -> Result<Arc<Self>, ApiError> {
    config.proxy.validate()?;
    let shell = Arc::new(RwLock::new(config.shell.resolve()?));
    tokio::fs::create_dir_all(&config.data_dir).await.map_err(ApiError::internal)?;
    let path = config.data_dir.join("wish.sqlite");
    let index_path = config.data_dir.join("management.sqlite");
    let (storage, index) = blocking(move || {
      Ok((
        Storage::open(path, StorageOptions::default())?,
        Arc::new(crate::server::management::ManagementStore::open(&index_path)?),
      ))
    })
    .await?;
    let tasks = TaskTracker::new();
    let mut providers = BTreeMap::new();
    for (id, provider_config) in &config.providers {
      let mut provider = Provider::build(provider_config.clone(), &config.proxy)?;
      provider.client = crate::server::sampling::observe_client(
        &provider.client,
        index.clone(),
        tasks.clone(),
        id.clone(),
        None,
      );
      providers.insert(id.clone(), Arc::new(provider));
    }
    let configuration =
      crate::server::configuration::Configuration::new(config_path, config.clone())?;
    let (events, _) = tokio::sync::broadcast::channel(256);
    let token = config
      .bearer_token_env
      .as_ref()
      .map(|key| crate::server::config::read_secret(key).map_err(ApiError::bad_request))
      .transpose()?;
    let web = if config.web.enabled {
      let provider = match config.web.provider {
        crate::server::config::WebProvider::DuckDuckGo => crate::tool::web::SearchProvider::DuckDuckGo,
        crate::server::config::WebProvider::Tavily => crate::tool::web::SearchProvider::Tavily,
        crate::server::config::WebProvider::Brave => crate::tool::web::SearchProvider::Brave,
      };
      Some(Arc::new(crate::tool::web::WebTool::new(crate::tool::web::WebConfig {
        provider,
        fetch_timeout: std::time::Duration::from_secs(config.web.fetch_timeout_secs.max(1)),
        api_key_env: None,
      })))
    } else {
      None
    };
    let mcp = Arc::new(crate::tool::mcp::McpRegistry::new());
    let mut mcp_specs = BTreeMap::new();
    let mut mcp_tools_map = BTreeMap::new();
    let mut mcp_http_map = BTreeMap::new();
    for (id, server) in &config.mcp_servers {
      // Capture each server's tool list once, at startup, so sessions never block on MCP I/O.
      let mut capture = |tools: Vec<crate::protocol::Tool>| {
        mcp_tools_map.insert(id.clone(), tools);
      };
      if let Some(endpoint) = &server.endpoint {
        // Remote server: reachable over streamable HTTP, no child process.
        let http = Arc::new(crate::tool::mcp::HttpMcpServer::new(
          endpoint.clone(),
          server.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
          std::time::Duration::from_secs(server.call_timeout_secs.max(1)),
        ));
        let mut tools = Vec::new();
        match http.tools_of().await {
          Ok(list) => {
            for mut tool in list {
              tool.name = format!("mcp_{id}_{}", tool.name);
              tools.push(tool);
            }
          }
          Err(error) => eprintln!("mcp http server {id}: tools/list failed: {error}"),
        }
        capture(tools);
        mcp_http_map.insert(id.clone(), http);
        continue;
      }
      if server.command.is_empty() {
        continue;
      }
      let spec = crate::tool::mcp::McpServerSpec {
        command: server.command.clone(),
        args: server.args.clone(),
        env: server.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        call_timeout: std::time::Duration::from_secs(server.call_timeout_secs.max(1)),
      };
      if let Err(error) = mcp.start(id, &spec).await {
        eprintln!("mcp server {id} failed to start: {error}");
      } else {
        let mut tools = Vec::new();
        if let Ok(list) = mcp.tools_of(id, &spec).await {
          for mut tool in list {
            tool.name = format!("mcp_{id}_{}", tool.name);
            tools.push(tool);
          }
        } else {
          eprintln!("mcp server {id}: tools/list failed");
        }
        capture(tools);
        mcp_specs.insert(id.clone(), spec);
      }
    }
    let mcp_specs = Arc::new(tokio::sync::Mutex::new(mcp_specs));
    let mcp_tools = std::sync::Mutex::new(mcp_tools_map);
    let mcp_http = std::sync::Mutex::new(mcp_http_map);
    let memory = if config.memory.enabled {
      match crate::session::memory::MemoryStore::open(config.data_dir.join("memory.sqlite")) {
        Ok(store) => Some(Arc::new(store)),
        Err(error) => {
          eprintln!("memory store failed to open: {error}");
          None
        }
      }
    } else {
      None
    };
    let subagents = Arc::new(crate::server::subagents::SubagentManager::new(
      config.subagents.max_concurrent,
      config.subagents.max_depth,
    ));
    let subagents_enabled = config.subagents.enabled;
    // The bundled web interface sits in a `web` folder beside the executable,
    // unless the configuration points somewhere else.
    let web_dir = config
      .web_dir
      .clone()
      .filter(|dir| dir.is_dir())
      .or_else(|| {
        std::env::current_exe()
          .ok()
          .and_then(|exe| exe.parent().map(|dir| dir.join("web")))
      })
      .filter(|dir| dir.is_dir());
    let app = Arc::new(Self {
      storage,
      index,
      configuration: AsyncMutex::new(configuration),
      codex_login: crate::server::codex_login::LoginManager::default(),
      events,
      started: std::time::Instant::now(),
      providers: RwLock::new(providers),
      shell,
      sessions: AsyncMutex::new(BTreeMap::new()),
      data_dir: config.data_dir.clone(),
      web_dir,
      token,
      stop: CancellationToken::new(),
      tasks,
      closing: Mutex::new(false),
      web,
      mcp,
      mcp_specs,
      mcp_tools,
      mcp_http,
      memory,
      subagents,
      subagents_enabled,
    });
    crate::server::codex_login::start_refresh_worker(&app);
    Ok(app)
  }
  pub fn get_provider(&self, id: &str) -> Result<Arc<Provider>, ApiError> {
    self
      .providers
      .read()
      .unwrap()
      .get(id)
      .filter(|p| p.config.enabled)
      .cloned()
      .ok_or_else(ApiError::not_found)
  }
  pub fn require_open(&self) -> Result<(), ApiError> {
    if *self.closing.lock().unwrap() {
      Err(ApiError::conflict("server is shutting down"))
    } else {
      Ok(())
    }
  }
  pub async fn create_session(
    self: &Arc<Self>,
    mut input: CreateSession,
  ) -> Result<Arc<SessionSlot>, ApiError> {
    self.require_open()?;
    self.get_provider(&input.provider)?;
    if !input.cwd.is_absolute() {
      return Err(ApiError::bad_request("cwd must be absolute"));
    }
    if !tokio::fs::metadata(&input.cwd)
      .await
      .map_err(|e| ApiError::bad_request(e.to_string()))?
      .is_dir()
    {
      return Err(ApiError::bad_request("cwd must be a directory"));
    }
    if !input.config.tools.is_empty() {
      return Err(ApiError::bad_request(
        "tools are installed by the server; pass an empty tools array",
      ));
    }
    let descriptor = Descriptor {
      pending_selection: None,
      id: uuid::Uuid::new_v4().to_string(),
      provider: input.provider,
      cwd: input.cwd,
      shell: input.shell,
      shell_command: None,
      created_at: crate::session::statistics::Timestamp::now().0,
      updated_at: crate::session::statistics::Timestamp::now().0,
      name: input.name,
      revision: 0,
    };
    for message in &mut input.initial_messages {
      message.normalize_new_input();
    }
    crate::server::media::apply_agent_instructions(&mut input.initial_messages);
    let storage = self.storage.clone();
    let id = descriptor.id.clone();
    let session = blocking(move || {
      let mut session = Session::create(storage, &id, input.config)?;
      session.set_metadata(input.metadata)?;
      session.import_history(input.initial_messages)?;
      Ok(session)
    })
    .await?;
    let slot = SessionSlot::open(
      descriptor.clone(),
      session,
      self.data_dir.clone(),
      self.index.clone(),
      self.events.clone(),
      Arc::downgrade(self),
    )
    .await?;
    {
      let mut session = slot.session.lock().await;
      let config = slot.configure_tools(session.get_config().clone())?;
      session.set_config(config)?;
      slot.update_snapshot(&session);
    }
    slot.persist_index()?;
    self.sessions.lock().await.insert(slot.get_descriptor().id.clone(), slot.clone());
    Ok(slot)
  }
  pub async fn get_session(self: &Arc<Self>, id: &str) -> Result<Arc<SessionSlot>, ApiError> {
    let mut sessions = self.sessions.lock().await;
    if let Some(slot) = sessions.get(id) {
      slot.require_live()?;
      return Ok(slot.clone());
    }
    let storage = self.storage.clone();
    let index = self.index.clone();
    let id = id.to_owned();
    let (descriptor, session) = blocking(move || {
      let value = index.read(&id)?;
      let descriptor =
        serde_json::from_value(value["session"].clone()).map_err(ApiError::internal)?;
      let session = Session::load(storage, &id)?;
      Ok((descriptor, session))
    })
    .await?;
    let slot = SessionSlot::open(
      descriptor,
      session,
      self.data_dir.clone(),
      self.index.clone(),
      self.events.clone(),
      Arc::downgrade(self),
    )
    .await?;
    {
      let mut session = slot.session.lock().await;
      if session.get_state().is_stable() {
        let config = slot.configure_tools(session.get_config().clone())?;
        session.set_config(config)?;
        slot.update_snapshot(&session);
      }
    }
    slot.persist_index()?;
    sessions.insert(slot.get_descriptor().id.clone(), slot.clone());
    Ok(slot)
  }
  pub async fn begin_shutdown(&self) {
    *self.closing.lock().unwrap() = true;
    self.stop.cancel();
    for slot in self.sessions.lock().await.values() {
      slot.interrupt();
    }
  }
  pub async fn finish_shutdown(&self) -> Result<(), ApiError> {
    self.tasks.close();
    self.tasks.wait().await;
    self.mcp.shutdown().await;
    for slot in self.sessions.lock().await.values() {
      if let Some(shell) = &slot.tools.shell {
        shell.shutdown().await.map_err(ApiError::internal)?;
      }
    }
    let storage = self.storage.clone();
    blocking(move || {
      storage.shutdown()?;
      Ok(())
    })
    .await
  }
}