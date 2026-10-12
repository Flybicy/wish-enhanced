//! The observational memory ledger, read and distilled through the API.
//!
//! Runs capture one bounded observation each (see `SessionSlot::observe_memory`);
//! reflections are the durable facts distilled from them, and both are what the
//! `memory_recall` tool searches. This surface answers the reader's questions —
//! how much memory there is, what it currently says, and what matches a query —
//! and lets a client distill a reflection of its own.
use crate::server::{app::App, error::{ApiError, blocking}};
use axum::{
  Json,
  extract::{Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// How much memory the installation holds, and the durable projection distilled from it.
pub async fn status(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  let Some(memory) = app.memory.clone() else {
    return Ok(Json(json!({"enabled": false})));
  };
  let (enabled, settings) = {
    let configuration = app.config_file.lock().await;
    (configuration.config.memory.enabled, configuration.config.memory.clone())
  };
  blocking(move || {
    let (observations, reflections) = memory.counts();
    let recent: Vec<Value> = memory
      .reflections(20)
      .into_iter()
      .map(|reflection| {
        json!({
          "id": reflection.id,
          "content": reflection.content,
          "derived_from": reflection.derived_from,
          "created_at": reflection.created_at,
        })
      })
      .collect();
    Ok(Json(json!({
      "enabled": enabled,
      "observations": observations,
      "reflections": reflections,
      "recent": recent,
      "projection": memory.projection(),
      "observe_after_tokens": settings.observe_after_tokens,
      "reflect_after_tokens": settings.reflect_after_tokens,
      "pool_target_tokens": settings.pool_target_tokens,
    })))
  })
  .await
}

#[derive(Deserialize)]
pub struct Search {
  q: String,
  #[serde(default = "default_limit")]
  limit: usize,
}
fn default_limit() -> usize {
  10
}
/// Substring search across observations and reflections.
pub async fn search(
  State(app): State<Arc<App>>,
  Query(query): Query<Search>,
) -> Result<Json<Value>, ApiError> {
  let Some(memory) = app.memory.clone() else {
    return Ok(Json(json!({"entries": [], "total": 0})));
  };
  if query.q.trim().is_empty() {
    return Err(ApiError::bad_request("q is required"));
  }
  let limit = query.limit.clamp(1, 50);
  let entries = blocking(move || memory.search(&query.q, limit).map_err(ApiError::internal))
    .await?;
  Ok(Json(json!({"entries": entries, "total": entries.len()})))
}

/// One reflection to distil, and the observations it came from.
#[derive(Deserialize)]
pub struct Reflection {
  pub content: String,
  #[serde(default)]
  pub derived_from: Vec<String>,
}
/// Appends a distilled reflection. Distillation is normally the reflector's job;
/// this is the same write, made by whoever holds the API.
pub async fn reflect(
  State(app): State<Arc<App>>,
  Json(input): Json<Reflection>,
) -> Result<Json<Value>, ApiError> {
  let Some(memory) = app.memory.clone() else {
    return Err(ApiError::bad_request("memory is not enabled"));
  };
  if input.content.trim().is_empty() {
    return Err(ApiError::bad_request("content is required"));
  }
  let id = blocking(move || {
    memory.reflect(&input.content, &input.derived_from).map_err(ApiError::internal)
  })
  .await?;
  Ok(Json(json!({"id": id})))
}
