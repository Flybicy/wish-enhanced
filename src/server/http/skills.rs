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
use std::path::{Path, PathBuf};
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

/// A skill to fetch and drop into a directory. `source` is an http(s) URL: it
/// may point at a markdown skill file, or at a `.zip` archive holding a directory
/// skill (a folder with SKILL.md plus its resources). `name` overrides the
/// installed name; `target` names which known directory receives it (default: the
/// built-in one).
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
/// a real skill is present, and refuses to overwrite. Only the built-in directory
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

  // Only the built-in directory and the configured extras are writable targets,
  // so a caller cannot steer a write outside the directories the page shows.
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
  let skill = crate::server::skills::Skill::parse(Path::new(&last_segment), &text)
    .ok_or_else(|| ApiError::bad_request("source is not a skill: missing '---' frontmatter"))?;
  let name = sanitize_name(requested.as_deref().unwrap_or(&skill.name))
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
    crate::server::skills::Skill::parse(&skill_md, &text)
      .ok_or_else(|| ApiError::bad_request("SKILL.md is not a skill: missing '---' frontmatter"))?;

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
    ApiError::internal(std::io::Error::new(
      error.kind(),
      format!("could not run the archive extractor ({}): {error}", program.display()),
    ))
  })?;
  if !output.status.success() {
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    let detail = if detail.is_empty() { "the extractor reported no detail" } else { detail };
    return Err(ApiError::bad_request(format!("could not extract the archive: {detail}")));
  }
  Ok(())
}

#[cfg(windows)]
fn tar_program() -> PathBuf {
  std::env::var_os("SystemRoot")
    .map(|root| PathBuf::from(root).join("System32").join("tar.exe"))
    .unwrap_or_else(|| PathBuf::from("tar.exe"))
}

#[cfg(not(windows))]
fn tar_program() -> PathBuf {
  PathBuf::from("tar")
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
  use super::{install_archive, locate_skill_root, looks_like_zip, sanitize_name, tar_program};

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

  #[test]
  fn detects_zip_signatures() {
    assert!(looks_like_zip(b"PK\x03\x04rest"));
    assert!(looks_like_zip(b"PK\x05\x06"));
    assert!(looks_like_zip(b"PK\x07\x08"));
    assert!(!looks_like_zip(b"---\nname: x\n"));
    assert!(!looks_like_zip(b"PK"));
    assert!(!looks_like_zip(b"PK\x01\x02"));
  }

  #[test]
  fn locates_skill_root_flat_and_nested() {
    let base = std::env::temp_dir().join(format!(
      "wish-skill-root-{}",
      std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    // Flat: SKILL.md at the unpack root.
    let flat = base.join("flat");
    std::fs::create_dir_all(&flat).unwrap();
    std::fs::write(flat.join("SKILL.md"), "---\nname: a\n---\nbody").unwrap();
    assert_eq!(locate_skill_root(&flat), Some(flat.clone()));

    // Nested: a single top folder that holds SKILL.md.
    let nested = base.join("nested");
    let inner = nested.join("my-skill");
    std::fs::create_dir_all(&inner).unwrap();
    std::fs::write(inner.join("SKILL.md"), "---\nname: b\n---\nbody").unwrap();
    assert_eq!(locate_skill_root(&nested), Some(inner));

    // No SKILL.md anywhere: rejected.
    let empty = base.join("empty");
    std::fs::create_dir_all(empty.join("a")).unwrap();
    std::fs::create_dir_all(empty.join("b")).unwrap();
    assert_eq!(locate_skill_root(&empty), None);

    let _ = std::fs::remove_dir_all(&base);
  }

  #[test]
  fn install_archive_unpacks_folder_skill() {
    let base = std::env::temp_dir().join(format!(
      "wish-archive-{}",
      std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    // A folder skill: SKILL.md with frontmatter plus a resource file beside it.
    let src = base.join("my-skill");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("SKILL.md"), "---\nname: packed\ndescription: from a zip\n---\nbody").unwrap();
    std::fs::write(src.join("res.txt"), "resource").unwrap();

    // Zip it with the same OS extractor the installer uses. GNU tar cannot write
    // zip, so skip the test where that build step is unavailable rather than fail.
    let zip = base.join("skill.zip");
    let built = std::process::Command::new(tar_program())
      .arg("-cf").arg(&zip).arg("--format").arg("zip")
      .arg("-C").arg(&base).arg("my-skill")
      .output();
    let can_zip = matches!(&built, Ok(out) if out.status.success()) && zip.is_file();
    if !can_zip {
      let _ = std::fs::remove_dir_all(&base);
      return;
    }

    let bytes = std::fs::read(&zip).unwrap();
    assert!(looks_like_zip(&bytes));

    let target = base.join("skills");
    let first = install_archive(&target, "installed", &bytes);
    assert!(first.is_ok(), "install failed: {:?}", first.err().map(|e| format!("{e:?}")));
    assert!(target.join("installed").join("SKILL.md").is_file());
    assert!(target.join("installed").join("res.txt").is_file());
    // Scratch directories are cleaned up.
    let leftovers = std::fs::read_dir(&target)
      .unwrap()
      .flatten()
      .any(|entry| entry.file_name().to_string_lossy().starts_with(".wish-install-"));
    assert!(!leftovers, "scratch directory was left behind");
    // A second install refuses to overwrite.
    assert!(install_archive(&target, "installed", &bytes).is_err());

    let _ = std::fs::remove_dir_all(&base);
  }
}
