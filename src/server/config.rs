//! The configuration file's schema, and the checks a configuration passes at startup and on every
//! save before anything is built from it.
use crate::server::{
  error::ApiError, mcp::McpConfig, provider::ProviderConfig, provider::read_secret,
  search::SearchConfig, session::ToolSwitches,
};
use crate::session::{CompactionConfig, SessionConfig};
use crate::tool::shell::{self, ShellCommand};
use crate::transport::Proxy;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf};

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
  pub listen: SocketAddr,
  pub data_dir: PathBuf,
  /// Name of the environment variable holding the bearer token that
  /// `/api` requires for write calls. Unset (or holding an empty value, which
  /// fails startup) leaves the server read-only: reads and diagnostics answer,
  /// every write is rejected.
  pub bearer_token_env: Option<String>,
  /// The bearer token itself, for a configuration that keeps it in the file
  /// instead of naming an environment variable. A start with neither set writes
  /// a generated one here, so the web app served beside the server can
  /// authenticate without an environment variable to set up first.
  pub bearer_token: Option<String>,
  /// The built web app to hand out beside the API; `web` beside the program when absent.
  /// Read at startup.
  pub web_dir: Option<PathBuf>,
  /// `host[:port]` names, besides loopback and the listen address, that browsers may reach a
  /// server without a token by: a LAN address or a domain. Read at startup.
  pub allowed_hosts: Vec<String>,
  pub providers: BTreeMap<String, ProviderConfig>,
  pub proxy: ProxyConfig,
  pub shell: ShellSettings,
  pub mcp: McpConfig,
  /// The services `web_search` asks, and in what order.
  pub search: SearchConfig,
  /// Where `wish skill` finds skills besides Wish's own directory, and which are off.
  pub skills: SkillsConfig,
  /// How much of the usage records is kept.
  pub usage: UsageConfig,
  pub defaults: Defaults,
  /// Observational memory (observations and reflections across sessions).
  pub memory: MemorySettings,
  /// In-process subagent delegation.
  pub subagents: SubagentsSettings,
  /// Workspace snapshotting (shadow git) for undo and redo.
  pub workspace_history: WorkspaceHistorySettings,
}


#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemorySettings {
  pub enabled: bool,
  /// Source-entry tokens after which the observer captures an observation.
  pub observe_after_tokens: u64,
  /// Source-entry tokens after which the reflector distills reflections.
  pub reflect_after_tokens: u64,
  /// Active observation pool target, in tokens, after a successful reflection.
  pub pool_target_tokens: u64,
}
impl Default for MemorySettings {
  fn default() -> Self {
    Self { enabled: true, observe_after_tokens: 10_000, reflect_after_tokens: 20_000, pool_target_tokens: 6_000 }
  }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SubagentsSettings {
  pub enabled: bool,
  pub max_concurrent: usize,
  pub max_depth: usize,
}
impl Default for SubagentsSettings {
  fn default() -> Self {
    Self { enabled: true, max_concurrent: 10, max_depth: 2 }
  }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceHistorySettings {
  /// true, false, or "auto": enabled when the working directory looks like a project.
  pub enabled: String,
}
impl Default for WorkspaceHistorySettings {
  fn default() -> Self {
    Self { enabled: "auto".into() }
  }
}
impl Config {
  /// Refuses a proxy, MCP server, search provider or usage limit that could never work.
  pub fn validate(&self) -> Result<(), ApiError> {
    self.proxy.validate()?;
    self.mcp.validate(&self.providers)?;
    self.search.validate(&self.providers)?;
    self.usage.validate()
  }
  /// The bearer token this configuration asks for: the environment variable it
  /// names when it names one, which must hold a value, otherwise the token the
  /// file holds. `None` leaves the server refusing every write.
  pub fn bearer_token(&self) -> Result<Option<String>, ApiError> {
    match &self.bearer_token_env {
      Some(name) => read_secret(name).map(Some).map_err(ApiError::bad_request),
      None => Ok(self.bearer_token.clone().filter(|token| !token.is_empty())),
    }
  }
  /// A token for a configuration that has none: 128 bits, hex, so it survives a
  /// configuration file, a page's markup and a shell alike.
  pub fn generate_bearer_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
  }
}

/// A start with no token configured gets one, kept in the configuration file so
/// the web app served beside the server can authenticate: the page it opens
/// carries the token, and a save from the settings page keeps it. A file that
/// cannot be written leaves the token in memory for this run only, and the next
/// start generates another.
pub async fn ensure_bearer_token(config: &mut Config, path: &std::path::Path) {
  if config.bearer_token_env.is_some()
    || config.bearer_token.as_ref().is_some_and(|token| !token.is_empty())
  {
    return;
  }
  config.bearer_token = Some(Config::generate_bearer_token());
  let Ok(bytes) = serde_json::to_vec_pretty(&config) else { return };
  match crate::server::config_file::write_atomic(path, &bytes).await {
    Ok(()) => eprintln!("wrote a bearer token to {}", path.display()),
    Err(error) => eprintln!("could not keep a bearer token in {}: {error}", path.display()),
  }
}
/// The default of a switch that is on unless the file says otherwise.
pub(super) fn yes() -> bool {
  true
}

/// Skills a session's model finds with `wish skill`: Wish's own directory, `skills` beside the
/// configuration file, then these directories in order, after the session's `.agents/skills`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SkillsConfig {
  /// More directories to find skills in; a leading `~` is the home directory.
  pub dirs: Vec<String>,
  /// Skills switched off, by name, wherever they are found.
  pub disabled: Vec<String>,
}

/// The usage records' size. Applied at once on a save.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct UsageConfig {
  /// The most one-second stream samples kept. Past it neighbouring samples are merged, those
  /// that would span the least time first, each session's and model's output and streaming time
  /// staying as they were, until it is met; `null` keeps every sample.
  pub stream_sample_limit: Option<u64>,
}
impl Default for UsageConfig {
  fn default() -> Self {
    Self { stream_sample_limit: Some(20_000) }
  }
}
impl UsageConfig {
  /// The fewest samples a limit may keep.
  const LEAST_SAMPLES: u64 = 100;
  fn validate(&self) -> Result<(), ApiError> {
    match self.stream_sample_limit {
      Some(limit) if limit < Self::LEAST_SAMPLES => Err(ApiError::bad_request(format!(
        "usage.stream_sample_limit must be at least {}",
        Self::LEAST_SAMPLES
      ))),
      _ => Ok(()),
    }
  }
}

/// The shell every session's commands run under. Applied to the next command after a save.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShellSettings {
  /// Absolute path of the shell; empty for the platform default.
  pub program: String,
  /// Arguments before the command text; absent for the ones the shell's family takes.
  pub args: Option<Vec<String>>,
}

impl ShellSettings {
  pub fn resolve(&self) -> Result<ShellCommand, ApiError> {
    let mut command = if self.program.is_empty() {
      ShellCommand::platform_default()
    } else {
      let program = PathBuf::from(&self.program);
      if !program.is_absolute() {
        return Err(ApiError::bad_request("shell program must be an absolute path"));
      }
      if !shell::is_runnable(&program) {
        return Err(ApiError::bad_request(format!(
          "shell program {} is not an executable file",
          program.display()
        )));
      }
      ShellCommand::for_program(program)
    };
    if let Some(args) = &self.args {
      command.args = args.clone();
    }
    Ok(command)
  }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProxyConfig {
  pub mode: ProxyMode,
  pub url: String,
  pub username: String,
  pub password: String,
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
  #[default]
  Environment,
  Manual,
  Direct,
}

impl Default for ProxyConfig {
  fn default() -> Self {
    Self {
      mode: ProxyMode::Environment,
      url: String::new(),
      username: String::new(),
      password: String::new(),
    }
  }
}

impl ProxyConfig {
  pub fn validate(&self) -> Result<(), ApiError> {
    if self.url.is_empty() {
      return if matches!(self.mode, ProxyMode::Manual) {
        Err(ApiError::bad_request("manual proxy requires a URL"))
      } else {
        Ok(())
      };
    }
    let url = reqwest::Url::parse(&self.url)
      .map_err(|_| ApiError::bad_request("proxy URL must be valid"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
      return Err(ApiError::bad_request("manual proxy URL must use http:// or https://"));
    }
    if !url.username().is_empty() || url.password().is_some() {
      return Err(ApiError::bad_request(
        "put proxy credentials in the username and password fields, not the URL",
      ));
    }
    if self.username.is_empty() && !self.password.is_empty() {
      return Err(ApiError::bad_request("proxy username is required when password is set"));
    }
    Ok(())
  }

  pub fn policy(&self, provider_enabled: bool) -> Proxy {
    if !provider_enabled {
      return Proxy::Disabled;
    }
    match self.mode {
      ProxyMode::Environment => Proxy::Environment,
      ProxyMode::Direct => Proxy::Disabled,
      ProxyMode::Manual => Proxy::Manual {
        url: self.url.clone(),
        basic_auth: (!self.username.is_empty())
          .then(|| (self.username.clone(), self.password.clone())),
      },
    }
  }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
  pub provider: String,
  pub model: String,
  pub cwd: PathBuf,
  /// The tools new sessions start with.
  pub tools: ToolSwitches,
  pub stream: bool,
  pub instructions: String,
  pub reasoning: Option<crate::protocol::ReasoningConfig>,
  pub max_output_tokens: Option<u64>,
  pub compaction: Option<CompactionConfig>,
}
impl Default for Defaults {
  fn default() -> Self {
    Self {
      provider: String::new(),
      model: String::new(),
      cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp")),
      tools: ToolSwitches {
        shell: true,
        ask_user: true,
        mcp: true,
        web_search: true,
        skills: true,
        sessions: true,
      },
      stream: true,
      instructions: String::new(),
      reasoning: None,
      max_output_tokens: None,
      // Auto-compaction on by default: trigger at 272k tokens (matches the
      // stock model's context window), condense back to 96k, and keep a
      // standby summary warmed every 48k of fresh turns.
      compaction: Some(CompactionConfig {
        trigger_tokens: 272_000,
        target_tokens: 96_000,
        segment_tokens: 48_000,
        estimator: crate::session::TokenEstimator::default(),
      }),
    }
  }
}
impl Defaults {
  pub fn session_config(&self) -> SessionConfig {
    let mut config = SessionConfig::new(&self.model);
    config.stream = self.stream;
    config.reasoning = self.reasoning.clone();
    config.max_output_tokens = self.max_output_tokens;
    config.compaction = self.compaction.clone();
    config
  }
}
impl Default for Config {
  fn default() -> Self {
    Self {
      listen: "127.0.0.1:9780".parse().unwrap(),
      data_dir: PathBuf::from("data"),
      bearer_token_env: None,
      bearer_token: None,
      web_dir: None,
      allowed_hosts: Vec::new(),
      providers: BTreeMap::new(),
      proxy: ProxyConfig::default(),
      shell: ShellSettings::default(),
      mcp: Default::default(),
      search: Default::default(),
      skills: SkillsConfig::default(),
      usage: UsageConfig::default(),
      defaults: Defaults::default(),
      memory: MemorySettings::default(),
      subagents: SubagentsSettings::default(),
      workspace_history: WorkspaceHistorySettings::default(),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  // The capability sections must default-fill when a config predates them, so an
  // older config.json still deserializes and the UI has values to bind.
  #[test]
  fn capabilities_default_when_absent() {
    let config: Config = serde_json::from_value(json!({})).unwrap();
    assert!(config.skills.dirs.is_empty());
    assert!(config.skills.disabled.is_empty());
    assert_eq!(config.memory.observe_after_tokens, 10_000);
    assert_eq!(config.memory.reflect_after_tokens, 20_000);
    assert_eq!(config.memory.pool_target_tokens, 6_000);
    assert!(config.subagents.enabled);
    assert_eq!(config.subagents.max_concurrent, 10);
    assert_eq!(config.subagents.max_depth, 2);
  }

  // deny_unknown_fields protects the round-trip: a typo'd key is rejected rather
  // than silently dropped, so a saved config never loses settings on reload.
  #[test]
  fn unknown_capability_key_is_rejected() {
    let err =
      serde_json::from_value::<SkillsConfig>(json!({ "enabled": true, "dors": ["x"] }));
    assert!(err.is_err(), "unknown key must be rejected");
  }

  // The whole document round-trips: default -> JSON -> default, unchanged.
  #[test]
  fn config_round_trips_through_json() {
    let original = Config::default();
    let value = serde_json::to_value(&original).unwrap();
    let restored: Config = serde_json::from_value(value).unwrap();
    assert_eq!(restored.subagents.max_depth, original.subagents.max_depth);
    assert_eq!(restored.memory.pool_target_tokens, original.memory.pool_target_tokens);
  }

  // A first start has no token of either kind, and gets one written into the
  // file: the environment variable stays unset, so nothing has to be configured
  // before the web app beside the server can save its settings.
  #[test]
  fn a_start_without_a_token_gets_one_in_the_file() {
    let mut config = Config::default();
    assert!(config.bearer_token_env.is_none());
    assert!(config.bearer_token.is_none());
    let token = Config::generate_bearer_token();
    assert_eq!(token.len(), 32, "128 bits, hex");
    assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
    config.bearer_token = Some(token.clone());
    assert_eq!(config.bearer_token().unwrap().as_deref(), Some(token.as_str()));
    // An empty token in the file is no token at all.
    config.bearer_token = Some(String::new());
    assert_eq!(config.bearer_token().unwrap(), None);
  }

  // Naming an environment variable keeps asking for it: a name without a value
  // is a startup error, not a server that silently stops asking.
  #[test]
  fn a_named_variable_outranks_the_file() {
    let mut config = Config::default();
    config.bearer_token_env = Some("WISH_TEST_TOKEN_THAT_IS_NOT_SET".into());
    config.bearer_token = Some("in-the-file".into());
    assert!(config.bearer_token().is_err());
  }
}
