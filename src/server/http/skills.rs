//! Skills over HTTP: what `wish skill` asks through the bridge, and the settings page's view of
//! them. A session's skills switch is enforced here and nowhere else.
use super::bridge;
use crate::server::{
  app::App,
  config::SkillsConfig,
  error::{ApiError, blocking},
  skills::{self, Root, Skill},
};
use axum::{
  Json,
  extract::{Path, Query, State},
  http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where the session a bridge request speaks for finds skills, when its skills switch is on.
async fn session_roots(
  app: &Arc<App>,
  id: &str,
  headers: &HeaderMap,
) -> Result<(Vec<Root>, SkillsConfig), ApiError> {
  let slot = bridge::session(app, id, headers).await?;
  // Said to the model through the command's output, so it knows why and who can change it.
  let descriptor = slot.get_descriptor();
  if !descriptor.tools.skills {
    return Err(ApiError::conflict(
      "Skills are disabled for this session. The user can enable them in the session's settings.",
    ));
  }
  let config = app.config_file.lock().await.config.skills.clone();
  Ok((skills::roots(Some(&descriptor.cwd), &app.skills_dir, &config), config))
}

fn summary(skill: &Skill) -> Value {
  json!({"name": skill.name, "description": skill.description, "category": skill.category})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsQuery {
  /// Words describing a task: the skills that fit it, best first, instead of all of them.
  query: Option<String>,
}
pub async fn session_list(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<SkillsQuery>,
  headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
  let (roots, config) = session_roots(&app, &id, &headers).await?;
  blocking(move || {
    let found = skills::discover(&roots);
    let usable = found.usable(&config);
    let shown = match query.query.as_deref().map(str::trim).filter(|query| !query.is_empty()) {
      Some(query) => skills::find(&usable, query),
      None => usable,
    };
    Ok(Json(json!({"skills": shown.into_iter().map(summary).collect::<Vec<_>>()})))
  })
  .await
}
pub async fn session_show(
  State(app): State<Arc<App>>,
  Path((id, name)): Path<(String, String)>,
  headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
  let (roots, config) = session_roots(&app, &id, &headers).await?;
  blocking(move || {
    let found = skills::discover(&roots);
    let usable = found.usable(&config);
    // A name the model gets slightly wrong in case still finds the skill.
    let skill = usable
      .iter()
      .find(|skill| skill.name == name)
      .or_else(|| usable.iter().find(|skill| skill.name.eq_ignore_ascii_case(&name)))
      .ok_or_else(ApiError::not_found)?;
    Ok(Json(json!(skills::show(skill).map_err(ApiError::internal)?)))
  })
  .await
}

/// For settings: every skill in Wish's own and the configured directories - those another of the
/// same name keeps out and those switched off included - and what could not be read.
pub async fn list(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  let config = app.config_file.lock().await.config.skills.clone();
  let roots = skills::roots(None, &app.skills_dir, &config);
  let own = app.skills_dir.clone();
  blocking(move || {
    let found = skills::discover(&roots);
    let skills: Vec<Value> = found
      .skills
      .iter()
      .zip(found.standings())
      .map(|(skill, standing)| {
        json!({
          "name": skill.name, "description": skill.description, "category": skill.category,
          "path": skill.path, "dir": skill.dir, "source": skill.source,
          "disabled": config.disabled.contains(&skill.name), "standing": standing,
        })
      })
      .collect();
    let roots: Vec<Value> = roots
      .iter()
      .map(|root| json!({"source": root.source, "dir": root.dir, "exists": root.dir.is_dir()}))
      .collect();
    let problems: Vec<Value> = found
      .problems
      .iter()
      .map(|(path, message)| json!({"path": path, "message": message}))
      .collect();
    Ok(Json(json!({"dir": own, "roots": roots, "skills": skills, "problems": problems})))
  })
  .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShowQuery {
  /// Which of the skills of the name, by its directory; the first when not given.
  dir: Option<std::path::PathBuf>,
}
/// For settings: a skill's instructions and files, whether sessions see it or not.
pub async fn show(
  State(app): State<Arc<App>>,
  Path(name): Path<String>,
  Query(query): Query<ShowQuery>,
) -> Result<Json<Value>, ApiError> {
  let config = app.config_file.lock().await.config.skills.clone();
  let roots = skills::roots(None, &app.skills_dir, &config);
  blocking(move || {
    let found = skills::discover(&roots);
    let skill = found
      .skills
      .iter()
      .find(|skill| skill.name == name && query.dir.as_ref().is_none_or(|dir| skill.dir == *dir))
      .ok_or_else(ApiError::not_found)?;
    Ok(Json(json!(skills::show(skill).map_err(ApiError::internal)?)))
  })
  .await
}

/// A skill to fetch and drop into a directory. `source` is an http(s) URL: it
/// may point at a markdown skill file, or at a `.zip` archive holding a directory
/// skill (a folder with SKILL.md plus its resources). `name` overrides the
/// installed name; `target` names which known directory receives it (default:
/// Wish's own).
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRequest {
  pub source: String,
  #[serde(default)]
  pub name: Option<String>,
  #[serde(default)]
  pub target: Option<String>,
}

/// Fetches a skill from an http(s) URL and writes it into a known directory. A
/// zip archive is unpacked into a folder skill; anything else is treated as a
/// single markdown file. It validates the URL scheme, caps the download, confirms
/// a real skill is present, and refuses to overwrite. Only Wish's own directory
/// and the configured extras are writable targets, so a caller cannot steer a
/// write outside the directories the settings page shows.
pub async fn install(
  State(app): State<Arc<App>>,
  Json(request): Json<InstallRequest>,
) -> Result<Json<Value>, ApiError> {
  const MAX_MARKDOWN_BYTES: usize = 1 << 20; // 1 MiB: a prompt skill.
  const MAX_ARCHIVE_BYTES: usize = 32 << 20; // 32 MiB: a skill folder in a zip.

  let url = reqwest::Url::parse(request.source.trim())
    .map_err(|_| ApiError::bad_request("source is not a valid URL"))?;
  if !matches!(url.scheme(), "http" | "https") {
    return Err(ApiError::bad_request("only http and https sources are allowed"));
  }

  let client = reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(60))
    .redirect(reqwest::redirect::Policy::limited(5))
    .user_agent(concat!("wish/", env!("CARGO_PKG_VERSION")))
    .build()
    .map_err(ApiError::internal)?;
  let response = client.get(url.clone()).send().await.map_err(ApiError::internal)?;
  if !response.status().is_success() {
    return Err(ApiError::bad_request(format!("source returned HTTP {}", response.status().as_u16())));
  }
  let bytes = response.bytes().await.map_err(ApiError::internal)?;
  if bytes.len() > MAX_ARCHIVE_BYTES {
    return Err(ApiError::bad_request("source is larger than 32 MiB"));
  }

  // Only Wish's own directory and the configured extras are writable targets,
  // so a caller cannot steer a write outside the directories the page shows.
  let builtin = app.skills_dir.clone();
  let extra = {
    let configuration = app.config_file.lock().await;
    configuration.config.skills.dirs.clone()
  };
  let target = match request.target.as_deref().map(str::trim).filter(|dir| !dir.is_empty()) {
    None => builtin.clone(),
    Some(dir) => {
      let dir = PathBuf::from(crate::server::skills::expand_home(dir));
      if dir == builtin || extra.iter().any(|known| crate::server::skills::expand_home(known) == dir) {
        dir
      } else {
        return Err(ApiError::bad_request("target is not one of the configured skill directories"));
      }
    }
  };

  // The URL's last path segment feeds the name fallback.
  let last_segment = url
    .path_segments()
    .and_then(|mut segments| segments.next_back())
    .filter(|segment| !segment.is_empty())
    .unwrap_or("skill")
    .to_owned();
  let requested = request.name.as_deref().map(str::trim).filter(|name| !name.is_empty()).map(str::to_owned);

  // A zip carries a directory skill; anything else is a single markdown file.
  if looks_like_zip(&bytes) {
    let stem = last_segment.strip_suffix(".zip").unwrap_or(&last_segment);
    let name = sanitize_name(requested.as_deref().unwrap_or(stem))
      .ok_or_else(|| ApiError::bad_request("skill name has no usable characters"))?;
    let archive = bytes.to_vec();
    return blocking(move || install_archive(&target, &name, &archive)).await;
  }

  if bytes.len() > MAX_MARKDOWN_BYTES {
    return Err(ApiError::bad_request("source is larger than 1 MiB; expected a markdown skill"));
  }
  let text = String::from_utf8(bytes.to_vec())
    .map_err(|_| ApiError::bad_request("source is not UTF-8 text"))?;
  if !crate::server::skills::is_skill_text(&text) {
    return Err(ApiError::bad_request("source is not a skill: missing '---' frontmatter"));
  }
  let fallback = crate::server::skills::declared_name(&text)
    .or_else(|| sanitize_name(&last_segment).map(str::to_owned))
    .unwrap_or_else(|| "skill".into());
  let name = sanitize_name(requested.as_deref().unwrap_or(&fallback))
    .ok_or_else(|| ApiError::bad_request("skill name has no usable characters"))?;

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

/// A zip archive begins with the "PK" signature: a local file record (03 04), an
/// empty central directory (05 06), or a spanned marker (07 08).
fn looks_like_zip(bytes: &[u8]) -> bool {
  bytes.len() >= 4 && &bytes[..2] == b"PK" && matches!((bytes[2], bytes[3]), (3, 4) | (5, 6) | (7, 8))
}

/// Unpacks a downloaded zip into `target/<name>/`. It stages the archive in a
/// scratch directory beside the target, unpacks it with the OS extractor,
/// confirms a SKILL.md is present and that its folder did not escape the scratch
/// tree, then moves the skill into place. Refuses to overwrite an existing skill.
fn install_archive(target: &Path, name: &str, archive: &[u8]) -> Result<Json<Value>, ApiError> {
  std::fs::create_dir_all(target).map_err(ApiError::internal)?;
  let dest = target.join(name);
  if dest.exists() {
    return Err(ApiError::conflict(format!("a skill named '{name}' already exists in that directory")));
  }

  // Scratch lives beside the target so the final move is a same-volume rename.
  let stamp = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|elapsed| elapsed.as_nanos())
    .unwrap_or(0);
  let scratch = target.join(format!(".wish-install-{name}-{stamp}"));
  let outcome = (|| {
    let unpack = scratch.join("unpacked");
    std::fs::create_dir_all(&unpack).map_err(ApiError::internal)?;
    let zip_path = scratch.join("archive.zip");
    std::fs::write(&zip_path, archive).map_err(ApiError::internal)?;
    extract_zip(&zip_path, &unpack)?;

    let root = locate_skill_root(&unpack)
      .ok_or_else(|| ApiError::bad_request("archive has no SKILL.md at its root or in a single top folder"))?;
    // Guard against a symlink or traversal entry that points the skill folder
    // outside the unpack tree before we move it into the skills directory.
    let unpack_root = std::fs::canonicalize(&unpack).map_err(ApiError::internal)?;
    let root_real = std::fs::canonicalize(&root).map_err(ApiError::internal)?;
    if !root_real.starts_with(&unpack_root) {
      return Err(ApiError::bad_request("archive skill folder escaped the unpack directory"));
    }
    let skill_md = root.join("SKILL.md");
    let text = std::fs::read_to_string(&skill_md).map_err(ApiError::internal)?;
    if !crate::server::skills::is_skill_text(&text) {
      return Err(ApiError::bad_request("SKILL.md is not a skill: missing '---' frontmatter"));
    }

    std::fs::rename(&root, &dest).map_err(ApiError::internal)?;
    Ok(Json(json!({
      "name": name,
      "path": dest.display().to_string(),
      "directory": target.display().to_string(),
    })))
  })();
  let _ = std::fs::remove_dir_all(&scratch);
  outcome
}

/// The skill folder inside an unpacked archive: the unpack root when it holds a
/// SKILL.md directly, or its sole top-level subfolder when that one does. Any
/// other shape (no SKILL.md, or several top folders) returns None.
fn locate_skill_root(dir: &Path) -> Option<PathBuf> {
  if dir.join("SKILL.md").is_file() {
    return Some(dir.to_owned());
  }
  let mut subdirs = std::fs::read_dir(dir)
    .ok()?
    .flatten()
    .map(|entry| entry.path())
    .filter(|path| path.is_dir());
  let first = subdirs.next()?;
  if subdirs.next().is_none() && first.join("SKILL.md").is_file() {
    return Some(first);
  }
  None
}

/// Unpacks a zip into `dest` with the OS-provided extractor: Windows ships
/// tar.exe (bsdtar/libarchive, which reads zip and refuses path-traversal
/// entries) in System32; other platforms use `tar` on PATH. No zip crate is
/// pulled in, keeping the build lean.
fn extract_zip(zip: &Path, dest: &Path) -> Result<(), ApiError> {
  let program = tar_program();
  let mut command = std::process::Command::new(&program);
  command.arg("-xf").arg(zip).arg("-C").arg(dest);
  #[cfg(windows)]
  {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
  }
  let output = command.output().map_err(|error| {
    ApiError::internal(format!("could not run the archive extractor ({program}): {error}"))
  })?;
  if !output.status.success() {
    return Err(ApiError::bad_request(format!(
      "the archive extractor failed: {}",
      String::from_utf8_lossy(&output.stderr).trim()
    )));
  }
  Ok(())
}

/// The archive extractor: Windows' built-in tar.exe, or tar on PATH elsewhere.
fn tar_program() -> String {
  #[cfg(windows)]
  {
    let system = std::env::var_os("SYSTEMROOT")
      .map(std::path::PathBuf::from)
      .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
    let tar = system.join("System32").join("tar.exe");
    if tar.is_file() {
      return tar.display().to_string();
    }
  }
  "tar".into()
}

/// Keeps the letters, digits, dashes and underscores of a name; lowercase, with
/// runs of other characters as single dashes. None when nothing usable is left.
fn sanitize_name(name: &str) -> Option<String> {
  let mut cleaned = String::new();
  let mut dash = false;
  for character in name.chars() {
    if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
      if dash && !cleaned.is_empty() {
        cleaned.push('-');
      }
      dash = false;
      cleaned.push(character.to_ascii_lowercase());
    } else {
      dash = true;
    }
  }
  let trimmed = cleaned.trim_matches('-').to_owned();
  (!trimmed.is_empty()).then_some(trimmed)
}
