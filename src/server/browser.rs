//! Showing the web interface to whoever started Wish.
//!
//! A release build is a GUI-subsystem executable on Windows: double-clicked
//! from Explorer it owns no console, so the `open http://...` line nobody can
//! read is not a way to present the interface. When the process has no console
//! the web interface is opened in the default browser instead, and a failure to
//! start is reported in a message box rather than a window that never appears.
//!
//! Started from a terminal the process keeps the console behaviour: it prints
//! the address and leaves the browser alone. `--no-browser` opts out of the
//! automatic open in every case.

#[cfg(not(windows))]
use std::io::IsTerminal as _;
#[cfg(not(windows))]
use std::process::Command;

/// Whether this process has a console to talk to. A double-clicked release
/// build on Windows has none, and neither has a process whose output is piped
/// somewhere for a log.
///
/// Windows is asked about the console window rather than the terminal: a start
/// from a script with its output redirected still owns a console, and a
/// browser must not appear in the middle of that.
pub(crate) fn has_console() -> bool {
  #[cfg(windows)]
  {
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    // SAFETY: a plain query, no pointers involved.
    unsafe { !GetConsoleWindow().is_null() }
  }
  #[cfg(not(windows))]
  {
    std::io::stdout().is_terminal()
  }
}

/// Opens `url` the way the platform opens things.
pub(crate) fn open(url: &str) -> std::io::Result<()> {
  #[cfg(windows)]
  {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let file: Vec<u16> = std::ffi::OsStr::new(url)
      .encode_wide()
      .chain(std::iter::once(0))
      .collect();
    let verb = windows_sys::core::w!("open");
    // SAFETY: `file` is a NUL-terminated wide string that outlives the call and
    // every other argument is null; ShellExecuteW only reads them.
    let result = unsafe {
      ShellExecuteW(
        std::ptr::null_mut(),
        verb,
        file.as_ptr(),
        std::ptr::null(),
        std::ptr::null(),
        SW_SHOWNORMAL,
      )
    };
    // A value above 32 is success; below it is an SEERR_ code.
    if result as isize > 32 {
      Ok(())
    } else {
      Err(std::io::Error::other(format!(
        "the shell refused to open {url} (code {})",
        result as isize
      )))
    }
  }
  #[cfg(target_os = "macos")]
  {
    Command::new("open").arg(url).status().map(|_| ())
  }
  #[cfg(all(unix, not(target_os = "macos")))]
  {
    Command::new("xdg-open").arg(url).status().map(|_| ())
  }
}

/// Whether something on `address` already answers as Wish's web app. A start
/// that cannot bind because an earlier instance is serving is not a failure:
/// the interface the person wanted is already there.
pub(crate) async fn already_serving(address: std::net::SocketAddr) -> bool {
  let Ok(client) = reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(2))
    .build()
  else {
    return false;
  };
  let Ok(response) = client.get(format!("http://{address}/")).send().await else {
    return false;
  };
  if !response.status().is_success() {
    return false;
  }
  // The web app is a single page served from beside the executable; its index
  // is the only thing that answers here with the app's document.
  response
    .text()
    .await
    .map(|body| body.contains("<!doctype html"))
    .unwrap_or(false)
}

/// Reports a fatal error where the person who started Wish can see it: the
/// console when there is one, a message box when there is not.
pub(crate) fn report_failure(error: &str) {
  eprintln!("error: {error}");
  #[cfg(windows)]
  {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
      MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW,
    };
    let title: Vec<u16> = std::ffi::OsStr::new("Wish")
      .encode_wide()
      .chain(std::iter::once(0))
      .collect();
    let text: Vec<u16> = std::ffi::OsStr::new(error)
      .encode_wide()
      .chain(std::iter::once(0))
      .collect();
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
      MessageBoxW(
        std::ptr::null_mut(),
        text.as_ptr(),
        title.as_ptr(),
        MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
      );
    }
  }
}
