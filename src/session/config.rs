use crate::protocol::model_use::{
  request::{PromptCache, ReasoningConfig},
  tool::Tool,
};

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default)]
pub enum ToolMode {
  #[default]
  Serial,
  Parallel,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct RunOptions {
  pub tools: ToolMode,
}

/// How side-effect tools may run. Operate preserves the classic behavior; Ask
/// waits for the user's approval and ReadOnly refuses them outright.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
  #[default]
  Operate,
  Ask,
  ReadOnly,
}

impl PermissionMode {
  pub fn is_default(&self) -> bool {
    *self == Self::default()
  }
}
/// Session settings. Conversation is resolved from the active generation for each call.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
  pub model: String,
  pub stream: bool,
  pub tools: Vec<Tool>,
  pub max_output_tokens: Option<u64>,
  pub reasoning: Option<ReasoningConfig>,
  pub cache: Option<PromptCache>,
  pub run: RunOptions,
  /// Gates side-effect tools; unlike the rest of the config it may change mid-run.
  #[serde(default, skip_serializing_if = "PermissionMode::is_default")]
  pub permission: PermissionMode,
  #[serde(default)]
  pub compaction: Option<super::CompactionConfig>,
  /// A persistent objective, injected as a pinned Developer instruction at the head of every
  /// request so it survives compaction and context clears. None or empty means no goal.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub goal: Option<String>,
}

impl SessionConfig {
  pub fn new(model: impl Into<String>) -> Self {
    Self {
      model: model.into(),
      stream: true,
      tools: Vec::new(),
      max_output_tokens: None,
      reasoning: None,
      cache: None,
      run: RunOptions::default(),
      permission: PermissionMode::default(),
      compaction: None,
      goal: None,
    }
  }
}

use super::{Session, SessionError, SessionEvent};
use serde_json::Value;
impl Session {
  pub fn get_metadata(&self) -> &Value {
    &self.record.metadata
  }
  pub fn set_metadata(&mut self, value: Value) -> Result<(), SessionError> {
    self.update(move |transaction| {
      transaction.record.metadata = value.clone();
      transaction.record_event(SessionEvent::MetadataUpdated(value))
    })
  }
  pub fn get_config(&self) -> &SessionConfig {
    &self.record.config
  }
  pub fn set_config(&mut self, config: SessionConfig) -> Result<(), SessionError> {
    self.require_stable()?;
    if let Some(compaction) = &config.compaction {
      compaction.validate()?;
    }
    self.update(move |transaction| {
      for id in [transaction.record.active, transaction.record.standby] {
        let mut generation = transaction.load_generation(id)?;
        generation.config = config.clone();
        transaction.save_generation(&generation)?;
      }
      transaction.record.config = config.clone();
      transaction.record_event(SessionEvent::ConfigUpdated(Box::new(config)))
    })
  }
  /// Switches the permission mode. Unlike set_config it is safe on a busy
  /// session: it only rewrites the mode, so it never disturbs a run's settings.
  pub fn set_permission(&mut self, mode: PermissionMode) -> Result<(), SessionError> {
    self.update(move |transaction| {
      for id in [transaction.record.active, transaction.record.standby] {
        let mut generation = transaction.load_generation(id)?;
        generation.config.permission = mode;
        transaction.save_generation(&generation)?;
      }
      let mut config = transaction.record.config.clone();
      config.permission = mode;
      transaction.record.config = config.clone();
      transaction.record_event(SessionEvent::ConfigUpdated(Box::new(config)))
    })
  }
}

#[cfg(test)]
mod tests {
  use super::{PermissionMode, SessionConfig};

  #[test]
  fn permission_wire_values_round_trip() {
    for (mode, wire) in [
      (PermissionMode::Operate, "operate"),
      (PermissionMode::Ask, "ask"),
      (PermissionMode::ReadOnly, "read_only"),
    ] {
      assert_eq!(serde_json::to_string(&mode).unwrap(), format!("\"{wire}\""));
      assert_eq!(serde_json::from_str::<PermissionMode>(&format!("\"{wire}\"")).unwrap(), mode);
    }
    assert!(serde_json::from_str::<PermissionMode>("\"bogus\"").is_err());
  }

  #[test]
  fn legacy_config_defaults_to_operate() {
    let legacy = r#"{"model":"m","stream":true,"tools":[],"max_output_tokens":null,"reasoning":null,"cache":null,"run":{"tools":"Serial"}}"#;
    let config: SessionConfig = serde_json::from_str(legacy).unwrap();
    assert_eq!(config.permission, PermissionMode::Operate);
    let round: SessionConfig = serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
    assert_eq!(round.permission, PermissionMode::Operate);
  }

  #[test]
  fn default_permission_is_not_serialized() {
    let value = serde_json::to_value(SessionConfig::new("m")).unwrap();
    assert!(value.get("permission").is_none());
    let mut asked = value.as_object().cloned().unwrap();
    asked.insert("permission".into(), serde_json::json!("ask"));
    let config: SessionConfig = serde_json::from_value(serde_json::Value::Object(asked)).unwrap();
    assert_eq!(config.permission, PermissionMode::Ask);
  }
}
