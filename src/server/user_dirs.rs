//! Where Wish keeps its configuration and data when started without `--config`: the platform's
//! usual places for a user's settings and data, under `wish-agent`. The first such start writes a
//! configuration that listens on loopback and keeps its data beside it.
use serde_json::json;
use std::path::PathBuf;

const NAME: &str = "wish-agent";
/// The address a new configuration listens on: loopback, on the port the web app has always used.
const LISTEN: &str = "127.0.0.1:8790";

/// The configuration file, written first if there is none yet.
pub fn ensure_config() -> Result<PathBuf, String> {
  if let Some(path) = try_locations() {
    return Ok(path);
  }
  // The user profile is unwritable (locked-down machine, AV, a profile dir
  // created by an elevated run). A portable install must still start: keep
  // the configuration and its data beside the executable instead.
  let exe_dir = std::env::current_exe()
    .ok()
    .and_then(|exe| exe.parent().map(PathBuf::from))
    .ok_or("cannot locate the executable for a portable configuration; pass --config")?;
  let portable = exe_dir.join(NAME);
  let path = portable.join("config.json");
  if path.exists() {
    return Ok(path);
  }
  std::fs::create_dir_all(&portable)
    .map_err(|error| format!("create {}: {error}", portable.display()))?;
  let data = portable.join("data");
  let text = serde_json::to_string_pretty(&json!({"listen": LISTEN, "data_dir": data}))
    .map_err(|error| error.to_string())?;
  std::fs::write(&path, format!("{text}\n"))
    .map_err(|error| format!("write {}: {error}", path.display()))?;
  eprintln!("created portable configuration {}", path.display());
  Ok(path)
}

/// The first writable user location: `None` when the profile cannot host a config.
fn try_locations() -> Option<PathBuf> {
  let (config_dir, data_dir) = locations().ok()?;
  let path = config_dir.join("config.json");
  if path.exists() {
    return Some(path);
  }
  let text = serde_json::to_string_pretty(&json!({"listen": LISTEN, "data_dir": data_dir}))
    .ok()?;
  if std::fs::create_dir_all(&config_dir).is_err() {
    return None;
  }
  match std::fs::write(&path, format!("{text}\n")) {
    Ok(()) => {
      eprintln!("created {}", path.display());
      Some(path)
    }
    Err(_) => None,
  }
}

/// The configuration's directory and the data directory a new configuration names.
fn locations() -> Result<(PathBuf, PathBuf), String> {
  let variable =
    |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
  if cfg!(windows) {
    let profile = variable("USERPROFILE");
    let roaming =
      variable("APPDATA").or_else(|| profile.as_ref().map(|home| home.join("AppData/Roaming")));
    let local = variable("LOCALAPPDATA").or_else(|| profile.map(|home| home.join("AppData/Local")));
    return match (roaming, local) {
      (Some(roaming), Some(local)) => Ok((roaming.join(NAME), local.join(NAME).join("data"))),
      _ => Err("neither APPDATA nor USERPROFILE is set; pass --config".into()),
    };
  }
  let home = variable("HOME").ok_or("HOME is not set; pass --config")?;
  if cfg!(target_os = "macos") {
    let dir = home.join("Library/Application Support").join(NAME);
    return Ok((dir.clone(), dir.join("data")));
  }
  let config = variable("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
  let data = variable("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"));
  Ok((config.join(NAME), data.join(NAME)))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn locations_derive_both_directories_from_the_profile() {
    let saved: Vec<_> = ["APPDATA", "LOCALAPPDATA", "USERPROFILE", "HOME"]
      .into_iter()
      .map(|name| (name, std::env::var_os(name)))
      .collect();
    // Throwaway paths: on Windows the profile variables alone must decide
    // where the configuration and the data directory live.
    unsafe {
      std::env::set_var("APPDATA", "E:/wish-test/roaming");
      std::env::set_var("LOCALAPPDATA", "E:/wish-test/local");
      std::env::set_var("USERPROFILE", "E:/wish-test");
    }
    let (config, data) = locations().expect("a profile");
    if cfg!(windows) {
      assert_eq!(config, PathBuf::from("E:/wish-test/roaming/wish-agent"));
      assert_eq!(data, PathBuf::from("E:/wish-test/local/wish-agent/data"));
    }
    for (name, value) in saved {
      match value {
        Some(value) => unsafe { std::env::set_var(name, value) },
        None => unsafe { std::env::remove_var(name) },
      }
    }
  }
}
