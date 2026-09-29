//! Workspace browsing for the interface's right rail: the working directory a
//! session runs in, listed one level at a time.
//!
//! The rail shows the same files the agent's shell sees, so the paths it lists
//! are exactly the paths the model can open. Only directories and files inside
//! the session's own working directory are reachable, and each request names
//! the directory explicitly rather than walking the tree on the server.
use crate::server::{app::App, error::{ApiError, blocking}};
use axum::{
  Json,
  extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listing {
  /// Directory to list, relative to the session's working directory. Empty
  /// means the working directory itself.
  #[serde(default)]
  path: String,
  #[serde(default)]
  limit: Option<usize>,
}

const DEFAULT_LIMIT: usize = 500;

/// Reject anything that could step outside the session's working directory.
fn resolve(root: &std::path::Path, relative: &str) -> Result<std::path::PathBuf, ApiError> {
  if relative.contains('\0') {
    return Err(ApiError::bad_request("invalid path"));
  }
  let candidate = if relative.is_empty() {
    root.to_owned()
  } else {
    let cleaned = relative.trim_start_matches(['/', '\\']);
    let mut result = root.to_owned();
    for part in cleaned.split(['/', '\\']) {
      if part.is_empty() || part == "." {
        continue;
      }
      if part == ".." {
        return Err(ApiError::bad_request("path escapes the working directory"));
      }
      // A drive-relative or absolute segment would replace the base entirely.
      if part.contains(':') {
        return Err(ApiError::bad_request("absolute paths are not allowed"));
      }
      result.push(part);
    }
    result
  };
  let canonical_root = root.canonicalize().map_err(|_| ApiError::bad_request("the working directory is unavailable"))?;
  let canonical = candidate.canonicalize().map_err(|_| ApiError::bad_request("no such directory"))?;
  if !canonical.starts_with(&canonical_root) {
    return Err(ApiError::bad_request("path escapes the working directory"));
  }
  Ok(canonical)
}

/// List one directory of the session's working space.
pub async fn list(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(listing): Query<Listing>,
) -> Result<Json<Value>, ApiError> {
  let session = app.get_session(&id).await?;
  let root = session.descriptor.read().unwrap().cwd.clone();
  if !root.is_dir() {
    return Err(ApiError::bad_request("the working directory no longer exists"));
  }
  let directory = resolve(&root, &listing.path)?;
  let limit = listing.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, 2000);
  let relative_base = listing.path.trim_matches(['/', '\\']).to_owned();

  let entries = blocking(move || -> Result<Value, ApiError> {
    let mut directories: Vec<Value> = Vec::new();
    let mut files: Vec<Value> = Vec::new();
    let reader = std::fs::read_dir(&directory).map_err(|error| ApiError::bad_request(error.to_string()))?;
    for entry in reader.flatten() {
      if directories.len() + files.len() >= limit {
        break;
      }
      let name = entry.file_name().to_string_lossy().into_owned();
      // Editor and system scratch files are noise in a workspace view.
      if name.starts_with('.') && name != ".env" && name != ".gitignore" {
        continue;
      }
      let Ok(kind) = entry.file_type() else { continue };
      let child = if relative_base.is_empty() {
        name.clone()
      } else {
        format!("{relative_base}/{name}")
      };
      if kind.is_dir() {
        directories.push(json!({"name": name, "path": child, "kind": "directory"}));
      } else if kind.is_file() {
        let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        files.push(json!({"name": name, "path": child, "kind": "file", "size": size}));
      }
    }
    directories.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    files.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let mut items = directories;
    items.extend(files);
    Ok(json!({"path": relative_base, "items": items, "truncated": items.len() >= limit}))
  })
  .await?;

  Ok(Json(entries))
}
