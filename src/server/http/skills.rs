//! The skill catalog the settings page shows: which directories contribute
//! skills, and what each of them holds.
//!
//! The engine already resolves skills at session start (server::skills, fed by
//! data_dir/skills plus the configured extras). This endpoint answers the same
//! question for the reader: it lists every directory in play and the skills
//! found in each, so the settings page can edit the directory list and show the
//! consequence. Reading is the catalog; writing is `install`, which fetches a
//! markdown skill from an http(s) URL and drops it into a known directory.
use crate::server::{app::App, error::{ApiError, blocking}};
use axum::{Json, extract::State};
use serde_json::{Value, json};
use std::path::PathBuf;
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

/// A skill to fetch and drop into a directory. The source is an http(s) URL to a
/// markdown skill file; the optional target names which known directory receives
/// it (default: the built-in one). name overrides the frontmatter name.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRequest {
  pub source: String,
  #[serde(default)]
  pub name: Option<String>,
  #[serde(default)]
  pub target: Option<String>,
}

/// Fetches a markdown skill from an http(s) URL and writes it into a known
/// directory. It validates the URL scheme, caps the download, parses the
/// frontmatter to confirm it is a real skill, and refuses to overwrite. Only the
/// built-in directory and the configured extras are writable targets, so a
/// caller cannot steer a write outside the directories the settings page shows.
pub async fn install(
  State(app): State<Arc<App>>,
  Json(request): Json<InstallRequest>,
) -> Result<Json<Value>, ApiError> {
  const MAX_BYTES: usize = 1 << 20; // 1 MiB: a prompt skill, not an archive.

  let url = reqwest::Url::parse(request.source.trim())
    .map_err(|_| ApiError::bad_request("source is not a valid URL"))?;
  if !matches!(url.scheme(), "http" | "https") {
    return Err(ApiError::bad_request("only http and https sources are allowed"));
  }

  let client = reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(30))
    .redirect(reqwest::redirect::Policy::limited(5))
    .user_agent(concat!("wish/", env!("CARGO_PKG_VERSION")))
    .build()
    .map_err(ApiError::internal)?;
  let response = client.get(url.clone()).send().await.map_err(ApiError::internal)?;
  if !response.status().is_success() {
    return Err(ApiError::bad_request(format!("source returned HTTP {}", response.status().as_u16())));
  }
  let bytes = response.bytes().await.map_err(ApiError::internal)?;
  if bytes.len() > MAX_BYTES {
    return Err(ApiError::bad_request("source is larger than 1 MiB; expected a markdown skill"));
  }
  let text = String::from_utf8(bytes.to_vec())
    .map_err(|_| ApiError::bad_request("source is not UTF-8 text"))?;

  // The URL's last path segment feeds the name fallback when the frontmatter
  // omits its own name.
  let fallback = url
    .path_segments()
    .and_then(|mut segments| segments.next_back())
    .filter(|segment| !segment.is_empty())
    .unwrap_or("skill");
  let skill = crate::server::skills::Skill::parse(std::path::Path::new(fallback), &text)
    .ok_or_else(|| ApiError::bad_request("source is not a skill: missing '---' frontmatter"))?;

  let raw_name = request.name.as_deref().map(str::trim).filter(|name| !name.is_empty())
    .unwrap_or(&skill.name);
  let name = sanitize_name(raw_name)
    .ok_or_else(|| ApiError::bad_request("skill name has no usable characters"))?;

  let builtin = app.data_dir.join("skills");
  let extra = {
    let configuration = app.configuration.lock().await;
    configuration.config.skills.dirs.clone()
  };
  let target = match request.target.as_deref().map(str::trim).filter(|dir| !dir.is_empty()) {
    None => builtin.clone(),
    Some(dir) => {
      let dir = PathBuf::from(dir);
      if dir == builtin || extra.iter().any(|known| *known == dir) {
        dir
      } else {
        return Err(ApiError::bad_request("target is not one of the configured skill directories"));
      }
    }
  };

  blocking(move || {
    std::fs::create_dir_all(&target).map_err(ApiError::internal)?;
    let path = target.join(format!("{name}.md"));
    if path.exists() {
      return Err(ApiError::conflict(format!("a skill named '{name}' already exists in that directory")));
    }
    std::fs::write(&path, text.as_bytes()).map_err(ApiError::internal)?;
    Ok(Json(json!({
      "name": name,
      "path": path.display().to_string(),
      "directory": target.display().to_string(),
    })))
  })
  .await
}

/// Reduces a name to a safe flat filename stem: keeps ASCII alphanumerics, dash
/// and underscore, folds every other run to a single dash, and trims dashes.
/// Returns None when nothing usable survives, so the caller can reject it.
fn sanitize_name(raw: &str) -> Option<String> {
  let mut out = String::with_capacity(raw.len());
  let mut pending_dash = false;
  for ch in raw.chars() {
    if ch.is_ascii_alphanumeric() || ch == '_' {
      if pending_dash && !out.is_empty() {
        out.push('-');
      }
      pending_dash = false;
      out.push(ch);
    } else {
      pending_dash = true;
    }
  }
  let trimmed = out.trim_matches('-');
  (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[cfg(test)]
mod tests {
  use super::sanitize_name;

  #[test]
  fn sanitize_keeps_word_characters() {
    assert_eq!(sanitize_name("office-docs"), Some("office-docs".into()));
    assert_eq!(sanitize_name("my_skill"), Some("my_skill".into()));
  }

  #[test]
  fn sanitize_folds_separators_and_strips_edges() {
    assert_eq!(sanitize_name("../../etc/passwd"), Some("etc-passwd".into()));
    assert_eq!(sanitize_name("  spaced name  "), Some("spaced-name".into()));
    assert_eq!(sanitize_name("a//b"), Some("a-b".into()));
  }

  #[test]
  fn sanitize_rejects_empty() {
    assert_eq!(sanitize_name(""), None);
    assert_eq!(sanitize_name("///"), None);
  }
}
