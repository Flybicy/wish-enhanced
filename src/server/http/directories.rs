use crate::server::error::{ApiError, blocking};
use axum::{Json, extract::Query};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Deserialize)]
pub struct DirectoryQuery {
  path: String,
}
#[derive(Serialize)]
pub struct DirectoryListing {
  path: String,
  parent: Option<String>,
  directories: Vec<String>,
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

pub async fn list(Query(query): Query<DirectoryQuery>) -> Result<Json<DirectoryListing>, ApiError> {
  blocking(move || {
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
    Ok(Json(DirectoryListing {
      parent: path.parent().map(|p| p.to_string_lossy().into_owned()),
      path: path.to_string_lossy().into_owned(),
      directories,
    }))
  })
  .await
}

#[cfg(test)]
mod tests {
  use super::display_path;
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
}
