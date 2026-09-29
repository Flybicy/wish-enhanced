//! The skill catalog the settings page shows: which directories contribute
//! skills, and what each of them holds.
//!
//! The engine already resolves skills at session start (server::skills, fed by
//! data_dir/skills plus the configured extras). This endpoint answers the same
//! question for the reader: it lists every directory in play and the skills
//! found in each, so the settings page can edit the directory list and show the
//! consequence. It is read-only — installing a skill is placing files, and the
//! settings page edits directories, not skill bodies.
use crate::server::{app::App, error::{ApiError, blocking}};
use axum::{Json, extract::State};
use serde_json::{Value, json};
use std::sync::Arc;

/// Every directory the engine reads skills from, with the skills each holds.
/// Grouped by directory so the settings page can edit the list and show the
/// effect of each entry side by side.
pub async fn catalog(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  let (enabled, extra) = {
    let configuration = app.configuration.lock().await;
    (configuration.config.skills.enabled, configuration.config.skills.dirs.clone())
  };
  let data_dir = app.data_dir.join("skills");
  blocking(move || {
    // The built-in directory always comes first: it is the one the installer
    // seeds and the reader cannot remove.
    let mut directories = vec![data_dir];
    directories.extend(extra);
    let mut groups = Vec::new();
    let mut names: Vec<Value> = Vec::new();
    let mut total = 0usize;
    for directory in &directories {
      let found = crate::server::skills::scan(std::slice::from_ref(directory));
      total += found.len();
      for skill in &found {
        names.push(json!({
          "name": skill.name,
          "description": skill.description,
          "triggers": skill.triggers,
          "path": skill.path.display().to_string(),
        }));
      }
      groups.push(json!({
        "path": directory.display().to_string(),
        "builtin": groups.is_empty(),
        "count": found.len(),
      }));
    }
    Ok(Json(json!({
      "enabled": enabled,
      "directories": groups,
      "skills": names,
      "total": total,
    })))
  })
  .await
}
