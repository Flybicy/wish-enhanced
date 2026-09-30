use crate::server::error::{ApiError, blocking};
use axum::{Json, extract::Query};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Deserialize)]
pub struct DirectoryQuery {
  // Empty (or absent) path is the sentinel for the roots view.
  #[serde(default)]
  path: String,
}
#[derive(Serialize, Deserialize)]
pub struct DirectoryListing {
  path: String,
  parent: Option<String>,
  directories: Vec<String>,
  #[serde(default)]
  root: bool,
}

// Windows `canonicalize` returns `\\?\`-prefixed verbatim paths. They are
// correct but hostile in a UI, and they break callers that join segments
// with forward slashes (verbatim paths reject them outright). Hand back a
// plain path whenever stripping the prefix is safe: short enough for the
// legacy Win32 limit, and free of the trailing dot or space that Win32
// would otherwise trim from a non-verbatim path.
fn display_path(path: PathBuf) -> PathBuf {
  let text = path.as_os_str().to_string_lossy().into_owned();
  let stripped = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
    Some(format!(r"\\{rest}"))
  } else {
    text.strip_prefix(r"\\?\").map(str::to_owned)
  };
  match stripped {
    Some(plain) if plain.len() < 248 && !plain.ends_with('.') && !plain.ends_with(' ') => PathBuf::from(plain),
    _ => path,
  }
}

// `X:\` style drive root: the one Windows path whose `parent()` is None
// that still has somewhere to climb to — the roots view.
fn is_drive_root(path: &str) -> bool {
  let bytes = path.as_bytes();
  bytes.len() == 3
    && bytes[0].is_ascii_alphabetic()
    && bytes[1] == b':'
    && (bytes[2] == b'\\' || bytes[2] == b'/')
}

// The roots view: every drive letter that answers a probe, so the picker can
// start above any single drive.
fn roots_listing() -> DirectoryListing {
  DirectoryListing {
    path: String::new(),
    parent: None,
    root: true,
    directories: ('A'..='Z')
      .filter(|letter| std::fs::metadata(format!("{letter}:\\")).is_ok())
      .map(|letter| format!("{letter}:\\"))
      .collect(),
  }
}

pub async fn list(Query(query): Query<DirectoryQuery>) -> Result<Json<DirectoryListing>, ApiError> {
  blocking(move || {
    if query.path.is_empty() {
      return Ok(Json(roots_listing()));
    }
    let path = if query.path == "~" {
      PathBuf::from(
        std::env::var_os("HOME")
          .or_else(|| std::env::var_os("USERPROFILE"))
          .ok_or_else(|| ApiError::bad_request("Server home directory is unavailable"))?,
      )
    } else {
      PathBuf::from(query.path)
    };
    if !path.is_absolute() {
      return Err(ApiError::bad_request("Directory path must be absolute"));
    }
    let path = display_path(
      path
        .canonicalize()
        .map_err(|error| ApiError::bad_request(format!("Cannot read directory: {error}")))?,
    );
    let entries = std::fs::read_dir(&path)
      .map_err(|error| ApiError::bad_request(format!("Cannot read directory: {error}")))?;
    let mut directories = Vec::new();
    for entry in entries {
      let entry =
        entry.map_err(|error| ApiError::bad_request(format!("Cannot read directory: {error}")))?;
      if entry.path().is_dir() {
        if let Some(name) = entry.file_name().to_str() {
          directories.push(name.to_owned());
        }
      }
    }
    directories.sort_unstable();
    let text = path.to_string_lossy().into_owned();
    let parent = match path.parent() {
      Some(parent) => Some(parent.to_string_lossy().into_owned()),
      // A drive root climbs to the roots view; UNC shares stay put.
      None if is_drive_root(&text) => Some(String::new()),
      None => None,
    };
    Ok(Json(DirectoryListing {
      parent,
      path: text,
      root: false,
      directories,
    }))
  })
  .await
}

#[derive(Deserialize)]
pub struct CreateDirectoryRequest {
  path: String,
  name: String,
}

#[derive(Serialize)]
pub struct CreatedDirectory {
  path: String,
}

// A folder name fit for joining: non-empty after trim, no path separators,
// no dot-only climb. Returns the trimmed name on success.
fn validate_directory_name(name: &str) -> Result<String, ApiError> {
  let trimmed = name.trim();
  if trimmed.is_empty() || trimmed == "." || trimmed == ".." || trimmed.contains('/') || trimmed.contains('\\') {
    return Err(ApiError::bad_request("Invalid directory name"));
  }
  Ok(trimmed.to_owned())
}

pub async fn create(Json(request): Json<CreateDirectoryRequest>) -> Result<Json<CreatedDirectory>, ApiError> {
  blocking(move || {
    let name = validate_directory_name(&request.name)?;
    let base = PathBuf::from(&request.path);
    if !base.is_absolute() {
      return Err(ApiError::bad_request("Directory path must be absolute"));
    }
    let joined = base.join(&name);
    std::fs::create_dir_all(&joined)
      .map_err(|error| ApiError::bad_request(format!("Cannot create directory: {error}")))?;
    let created = match joined.canonicalize() {
      Ok(canonical) => display_path(canonical),
      Err(_) => joined,
    };
    Ok(Json(CreatedDirectory { path: created.to_string_lossy().into_owned() }))
  })
  .await
}

#[cfg(test)]
mod tests {
  use super::{display_path, is_drive_root, roots_listing, validate_directory_name};
  use std::path::PathBuf;

  #[test]
  fn verbatim_drive_prefix_is_stripped() {
    assert_eq!(display_path(PathBuf::from(r"\\?\D:\Wish")), PathBuf::from(r"D:\Wish"));
  }

  #[test]
  fn verbatim_unc_prefix_becomes_a_plain_share() {
    assert_eq!(display_path(PathBuf::from(r"\\?\UNC\server\share")), PathBuf::from(r"\\server\share"));
  }

  #[test]
  fn long_verbatim_paths_stay_verbatim() {
    let long = format!(r"\\?\C:\{}", "x".repeat(300));
    assert_eq!(display_path(PathBuf::from(long.clone())), PathBuf::from(long));
  }

  #[test]
  fn drive_roots_are_recognized_for_parenting() {
    assert!(is_drive_root(r"C:\"));
    assert!(!is_drive_root(r"C:\Projects"));
    assert!(!is_drive_root(r"\\server\share"));
  }

  #[test]
  fn roots_listing_marks_root_and_lists_drives() {
    let listing = roots_listing();
    assert!(listing.root);
    assert!(listing.path.is_empty());
    assert!(listing.parent.is_none());
    assert!(listing.directories.iter().any(|entry| entry.contains(':')));
  }

  #[test]
  fn folder_names_are_validated() {
    for name in ["", "  ", ".", "..", "a/b", "a\\b"] {
      assert!(validate_directory_name(name).is_err(), "{name:?} must be rejected");
    }
    assert_eq!(validate_directory_name(" 新项目 ").unwrap(), "新项目");
  }
}
