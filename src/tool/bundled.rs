//! PATH preparation for child processes. The bundled tool folders next to
//! the executable (python, node, git, niubash) are prepended so children
//! resolve them without any installer writing user or system environment
//! variables.

use std::ffi::OsString;
use std::path::PathBuf;

// The inherited PATH with every bundled tool folder that exists next to the
// current executable prepended. None when nothing bundled sits there (dev
// builds), leaving the environment untouched.
pub fn child_path() -> Option<OsString> {
  let dir = exe_dir()?;
  let bundled: Vec<PathBuf> = [
    dir.join("niubash"),
    dir.join("python"),
    dir.join("python").join("Scripts"),
    dir.join("node"),
    dir.join("git").join("cmd"),
  ]
  .into_iter()
  .filter(|path| path.is_dir())
  .collect();
  if bundled.is_empty() {
    return None;
  }
  let inherited = std::env::var_os("PATH")?;
  let rest = std::env::split_paths(&inherited).filter(|path| !path.as_os_str().is_empty());
  std::env::join_paths(bundled.into_iter().chain(rest)).ok()
}

fn exe_dir() -> Option<PathBuf> {
  std::env::current_exe()
    .ok()
    .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
}
