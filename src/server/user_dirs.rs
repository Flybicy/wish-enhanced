//! Where Wish keeps its configuration and data when started without `--config`: the platform's
//! usual places for a user's settings and data, under `wish-agent`. The first such start writes a
//! configuration that listens on loopback and keeps its data beside it. Every candidate is tried
//! in turn, so an install into `Program Files` still starts when the user profile is locked down.
use serde_json::json;
use std::path::PathBuf;

const NAME: &str = "wish-agent";
/// The address a new configuration listens on: loopback, on the port the web app has always used.
const LISTEN: &str = "127.0.0.1:8790";

/// The configuration file, written first if there is none yet.
pub fn ensure_config() -> Result<PathBuf, String> {
  for (config_dir, data_dir) in candidates() {
    let path = config_dir.join("config.json");
    if path.exists() {
      return Ok(path);
    }
    let Ok(text) =
      serde_json::to_string_pretty(&json!({"listen": LISTEN, "data_dir": data_dir}))
    else {
      continue;
    };
    if std::fs::create_dir_all(&config_dir).is_err() {
      continue;
    }
    if std::fs::write(&path, format!("{text}\n")).is_ok() {
      eprintln!("created {}", path.display());
      return Ok(path);
    }
  }
  Err("no writable location for the configuration; pass --config".into())
}

/// Every location that could host the configuration, best first: the user's
/// roaming profile, then the local one (an install directory under
/// `Program Files` is not writable for a standard user), then a directory
/// beside the executable, and the temporary directory as a last resort.
fn candidates() -> Vec<(PathBuf, PathBuf)> {
  let variable =
    |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
  let mut out: Vec<(PathBuf, PathBuf)> = Vec::new();
  if cfg!(windows) {
    let profile = variable("USERPROFILE");
    let roaming =
      variable("APPDATA").or_else(|| profile.as_ref().map(|home| home.join("AppData/Roaming")));
    let local = variable("LOCALAPPDATA").or_else(|| profile.map(|home| home.join("AppData/Local")));
    if let (Some(roaming), Some(local)) = (roaming.as_ref(), local.as_ref()) {
      out.push((roaming.join(NAME), local.join(NAME).join("data")));
    }
    if let Some(local) = local.as_ref() {
      out.push((local.join(NAME), local.join(NAME).join("data")));
    }
  } else if let Some(home) = variable("HOME") {
    if cfg!(target_os = "macos") {
      let dir = home.join("Library/Application Support").join(NAME);
      out.push((dir.clone(), dir.join("data")));
    } else {
      let config = variable("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
      let data = variable("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"));
      out.push((config.join(NAME), data.join(NAME)));
    }
  }
  if let Ok(exe) = std::env::current_exe() {
    if let Some(dir) = exe.parent() {
      out.push((dir.join(NAME), dir.join(NAME).join("data")));
    }
  }
  let temporary = std::env::temp_dir();
  out.push((temporary.join(NAME), temporary.join(NAME).join("data")));
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn candidates_put_the_user_profile_first_on_windows() {
    let saved: Vec<_> = ["APPDATA", "LOCALAPPDATA", "USERPROFILE", "HOME"]
      .into_iter()
      .map(|name| (name, std::env::var_os(name)))
      .collect();
    // Throwaway paths: the roaming profile must win, with the data directory
    // kept beside the local one, exactly as the single-profile layout did.
    unsafe {
      std::env::set_var("APPDATA", "E:/wish-test/roaming");
      std::env::set_var("LOCALAPPDATA", "E:/wish-test/local");
      std::env::set_var("USERPROFILE", "E:/wish-test");
    }
    let all = candidates();
    if cfg!(windows) {
      assert_eq!(
        all.first(),
        Some(&(
          PathBuf::from("E:/wish-test/roaming/wish-agent"),
          PathBuf::from("E:/wish-test/local/wish-agent/data")
        ))
      );
      // The local profile, the executable's directory and the temporary
      // directory follow, so a locked-down machine still gets a start.
      assert!(all.len() >= 3, "{all:?}");
    }
    for (name, value) in saved {
      match value {
        Some(value) => unsafe { std::env::set_var(name, value) },
        None => unsafe { std::env::remove_var(name) },
      }
    }
  }
}
