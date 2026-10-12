//! The Wish server: one process serving the HTTP API and the web app, driving sessions on the
//! engine and calling providers.
//!
//! - `app` holds everything a request reaches, and the lifecycle from startup to shutdown;
//! - `http` is the API; its handlers parse, check and answer, and leave the work to the rest;
//! - `session` holds open sessions and runs them; `session_model` is the model a session calls;
//! - `config` is the configuration file's schema, `config_file` the file as the server holds it;
//! - `provider`, `search` and `mcp` are what a configuration builds: model providers, search
//!   providers and MCP servers;
//! - `management` is the index of sessions, model calls and stream samples (`sampling`), and
//!   `data_dir` and `blobs` are the files beside it;
//! - `web` hands out the web app beside the API and guards a server without a token;
//!   `user_dirs` is where a start without `--config` finds its configuration.

mod app;
mod blobs;
mod caption;
mod codex_login;
mod compaction_item;
mod config;
mod config_file;
#[cfg(windows)]
mod console;
mod data_dir;
mod error;
mod http;
mod management;
mod mcp;
mod model_catalog;
mod presets;
mod provider;
mod sampling;
mod search;
mod session;
mod session_model;
#[cfg(windows)]
mod shell_window;
mod skills;
mod subagents;
mod web;

use std::ffi::OsStr;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How the interface is presented once the HTTP service is up.
enum Shell {
  /// A chrome-less application window hosting the interface (the double-click default).
  Window,
  /// The system's default browser (opt-in with --browser).
  Browser,
  /// No presentation: a plain HTTP service (--no-window, or an explicit --config).
  Headless,
}

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
  let result = run_inner();
  // A double-clicked launch has no console: surface fatal startup problems in a
  // message box instead of exiting silently.
  if let Err(error) = &result {
    let text = format!("wish: {error}\n");
    cli_out(&text);
    alert(&text);
  }
  result
}

fn run_inner() -> Result<(), Box<dyn std::error::Error>> {
  let args: Vec<String> = std::env::args().skip(1).collect();
  // The MCP bridge speaks to a running server: wish mcp list|describe|call ...
  if args.first().map(String::as_str) == Some("mcp") {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let code = rt.block_on(mcp::cli::run(args[1..].to_vec()));
    std::process::exit(code);
  }
  let mut path: Option<PathBuf> = None;
  // A bare double-click opens the application window; any argument means an operator is present.
  let mut shell = if args.is_empty() { Shell::Window } else { Shell::Headless };
  let mut version = false;
  // A session to land on directly: --open <id> (or a pasted session URL).
  let mut open: Option<String> = None;
  // An operator override for the listen port, without editing config.json.
  let mut port: Option<u16> = None;
  let mut i = 0;
  while i < args.len() {
    match args[i].as_str() {
      "--config" => {
        i += 1;
        path = Some(PathBuf::from(
          args.get(i).ok_or("usage: wish [--config <config.json>] [--no-window]")?,
        ));
      }
      "--no-window" | "--no-browser" => shell = Shell::Headless,
      "--window" => shell = Shell::Window,
      "--browser" => shell = Shell::Browser,
      "--open" => {
        i += 1;
        open = Some(args.get(i).ok_or("usage: wish --open <session-id>")?.to_owned());
        shell = Shell::Window;
      }
      "--port" => {
        i += 1;
        port = Some(
          args
            .get(i)
            .ok_or("usage: wish --port <port>")?
            .parse()
            .map_err(|_| "usage: wish --port <port> (0-65535)")?,
        );
      }
      "--version" | "-V" => version = true,
      "--help" | "-h" => {
        cli_out(HELP_TEXT);
        return Ok(());
      }
      other => return Err(format!("unknown argument: {other}").into()),
    }
    i += 1;
  }
  if version {
    cli_out(&format!("wish {}\n", env!("CARGO_PKG_VERSION")));
    return Ok(());
  }
  let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
  let (config_path, generated) = rt.block_on(async {
    match path {
      Some(path) => Ok((path, false)),
      None => auto_config().await,
    }
  })?;
  if generated {
    cli_out(&format!("wish generated a default config at {}\n", config_path.display()));
  }
  // Older data directories are brought up to this version's layout first.
  crate::migration::run(&config_path)?;
  #[cfg(windows)]
  console::install();
  let mut config: config::Config = serde_json::from_slice(&std::fs::read(&config_path)?)?;
  if let Some(port) = port {
    config.listen.set_port(port);
  }
  let web_app = web::find_app_dir(config.web_dir.as_deref())?;
  let allowed_hosts = config.allowed_hosts.clone();
  let (app, listener) = rt.block_on(async {
    let app = app::App::open(&config, config_path.clone()).await?;
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    Ok::<_, Box<dyn std::error::Error>>((app, listener))
  })?;
  let addr = listener.local_addr()?;
  app.mcp.bridge.set_address(addr);
  let front = Arc::new(web::WebFront::new(web_app, addr, &allowed_hosts, app.bearer_token.is_some()));
  match shell {
    Shell::Headless => {
      cli_out(&format!("wish listening on http://{addr}\n"));
      if front.app_dir().is_some() {
        cli_out(&format!("open {} in a browser\n", host_url(addr)));
      }
      rt.block_on(serve(app, front, listener))?;
    }
    Shell::Window | Shell::Browser => {
      let server = std::thread::spawn(move || {
        let _ = rt.block_on(serve(app, front, listener));
      });
      if matches!(shell, Shell::Window) {
        let profile = config_path.parent().map(|dir| dir.join("window-profile"));
        #[cfg(windows)]
        {
          if let Some(dir) = &profile {
            let _ = std::fs::create_dir_all(dir);
          }
          match shell_window::run(addr, profile, open.clone()) {
            Ok(()) => std::process::exit(0),
            Err(error) => {
              let text = format!("wish could not open its window: {error}\n");
              cli_out(&text);
              alert(&text);
              open_system_browser(addr);
            }
          }
        }
        #[cfg(not(windows))]
        {
          let _ = profile;
          // No native WebView host exists off Windows yet: say the system
          // browser is the fallback instead of implying a window opened.
          cli_out(
            "wish has no native window on this platform yet; falling back to the system browser\n",
          );
          open_system_browser(addr);
        }
      } else {
        open_system_browser(addr);
      }
      // Serve until the window's host process (or the shutdown signal) ends.
      let _ = server.join();
    }
  }
  Ok(())
}

const HELP_TEXT: &str = "wish [--config <config.json>] [--port <port>] [--no-window] [--window] [--browser] [--open <session-id>]
wish mcp <list|describe|call> ... talks to a running server's MCP bridge
One HTTP service for providers and sessions, with a web interface.

Without --config, wish looks for config.json beside the executable, then in the
user's platform folder (Documents\\Wish on Windows, ~/Library/Application
Support/Wish on macOS, ~/.config/wish on Linux), generating a default one there
on first run.
--port overrides the listen port from the config for this run only.
Without other flags wish opens its interface in an application window;
--no-window serves headless and --browser uses the system browser instead.
--open jumps straight to one session (the interface opens there instead of the
session list).
";

/// Serve HTTP until the shutdown signal; the signal (Ctrl-C where a console
/// exists, or the console closing on Windows) drains sessions before exit.
async fn serve(
  app: Arc<app::App>,
  front: Arc<web::WebFront>,
  listener: tokio::net::TcpListener,
) -> Result<(), Box<dyn std::error::Error>> {
  let shutdown = app.clone();
  let result = axum::serve(listener, http::build_router(app.clone(), front))
    .with_graceful_shutdown(async move {
      wait_for_signal().await;
      shutdown.begin_shutdown().await;
    })
    .await;
  app.begin_shutdown().await;
  let finished = app.finish_shutdown().await;
  #[cfg(windows)]
  console::release();
  finished?;
  result?;
  Ok(())
}

/// Which platform's file conventions a build follows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
// Tests construct every variant on every host to pin the per-platform mapping,
// so a build only ever names its own — that is not dead code.
#[allow(dead_code)]
enum Platform {
  Windows,
  MacOs,
  Linux,
}

/// The platform this binary was built for. `cfg!` keeps this a compile-time
/// constant while the variant itself stays constructible for the unit tests.
fn current_platform() -> Platform {
  if cfg!(windows) {
    Platform::Windows
  } else if cfg!(target_os = "macos") {
    Platform::MacOs
  } else {
    Platform::Linux
  }
}

/// The user directory that holds config.json and the data directory. Windows
/// keeps the v0.3.x `Documents\Wish` location so existing installs keep their
/// data; macOS uses `~/Library/Application Support/Wish`; Linux follows XDG
/// (`$XDG_CONFIG_HOME/wish`, else `~/.config/wish`). The generated `data_dir`
/// always points inside this directory, never beside the executable.
fn user_dir_for(
  platform: Platform,
  home: Option<&OsStr>,
  xdg_config_home: Option<&OsStr>,
) -> Option<PathBuf> {
  match platform {
    Platform::Windows => Some(PathBuf::from(home?).join("Documents").join("Wish")),
    Platform::MacOs => {
      Some(PathBuf::from(home?).join("Library").join("Application Support").join("Wish"))
    }
    Platform::Linux => {
      let base = match xdg_config_home {
        // The XDG spec demands an absolute path; a relative value is ignored.
        Some(dir) if is_absolute_path(dir) => PathBuf::from(dir),
        _ => PathBuf::from(home?).join(".config"),
      };
      Some(base.join("wish"))
    }
  }
}

/// Absolute by POSIX (a leading `/`) or by the host's own rules (Windows
/// `C:\` drive paths). `Path::is_absolute` alone is host-dependent — it
/// rejects `/srv/cfg` on Windows — so the XDG mapping must not trust it for
/// the POSIX form, or the same function answers differently per OS.
fn is_absolute_path(path: &OsStr) -> bool {
  Path::new(path).is_absolute() || path.as_encoded_bytes().starts_with(b"/")
}

/// `user_dir_for` for the running platform, reading the real environment.
/// Windows prefers USERPROFILE (a git-bash style HOME lands somewhere else);
/// unix builds prefer HOME.
fn user_dir() -> Option<PathBuf> {
  let (first, second) =
    if cfg!(windows) { ("USERPROFILE", "HOME") } else { ("HOME", "USERPROFILE") };
  let home = std::env::var_os(first).or_else(|| std::env::var_os(second));
  user_dir_for(current_platform(), home.as_deref(), std::env::var_os("XDG_CONFIG_HOME").as_deref())
}

/// The config a first run writes into `dir`. `data_dir` stays inside the user
/// directory, and a bundled niubash beside the executable is wired in when the
/// installer left one there.
fn default_config(dir: &Path, niubash: Option<&Path>) -> serde_json::Value {
  let mut json = serde_json::json!({
    "listen": "127.0.0.1:8787",
    "data_dir": dir.join("data"),
  });
  if let Some(niu) = niubash {
    json["shell"] = serde_json::json!({ "program": niu, "args": ["-c"] });
  }
  json
}

/// Locate a configuration for double-click launches: beside the executable first,
/// then the per-platform user directory, generating a default one there on first
/// run.
async fn auto_config() -> Result<(PathBuf, bool), Box<dyn std::error::Error>> {
  let exe = std::env::current_exe()?;
  if let Some(dir) = exe.parent() {
    let beside = dir.join("config.json");
    if beside.exists() {
      return Ok((beside, false));
    }
  }
  let Some(dir) = user_dir() else {
    return Err("no --config given and no user home directory found".into());
  };
  let path = dir.join("config.json");
  if path.exists() {
    return Ok((path, false));
  }
  tokio::fs::create_dir_all(&dir).await?;
  // Sessions shell out through the bundled niubash when the installer placed it beside the executable.
  let niu = exe.parent().map(|p| p.join("niubash").join("niu.exe")).filter(|p| p.exists());
  let json = default_config(&dir, niu.as_deref());
  tokio::fs::write(&path, serde_json::to_string_pretty(&json)?).await?;
  Ok((path, true))
}

/// The URL an interface host loads: the loopback address the service listens on.
fn host_url(addr: SocketAddr) -> String {
  match addr.ip() {
    std::net::IpAddr::V4(v4) if v4.is_unspecified() => format!("http://127.0.0.1:{}", addr.port()),
    std::net::IpAddr::V6(v6) if v6.is_unspecified() => format!("http://[::1]:{}", addr.port()),
    ip => format!("http://{ip}:{}", addr.port()),
  }
}

/// The program and arguments that open `url` in the system browser: `start`
/// through cmd on Windows, `open` on macOS, `xdg-open` elsewhere. Pure, so the
/// mapping is unit-tested without spawning anything.
fn browser_command(platform: Platform, url: &str) -> (&'static str, Vec<String>) {
  match platform {
    Platform::Windows => {
      ("cmd", vec!["/C".to_owned(), "start".to_owned(), String::new(), url.to_owned()])
    }
    Platform::MacOs => ("open", vec![url.to_owned()]),
    Platform::Linux => ("xdg-open", vec![url.to_owned()]),
  }
}

/// Open the system's default browser on the listening address (best effort).
/// A failed launch still prints the URL so the interface stays reachable.
fn open_system_browser(addr: SocketAddr) {
  let url = host_url(addr);
  let (program, args) = browser_command(current_platform(), &url);
  let spawned = std::process::Command::new(program).args(&args).spawn();
  if let Err(error) = spawned {
    cli_out(&format!("wish could not open a browser ({error}); visit {url} manually\n"));
  }
}

/// Print operator-facing text. A windows-subsystem executable has no console:
/// piped or redirected handles still work, and an interactive terminal is
/// re-attached so --help and errors reach the person who asked for them.
fn cli_out(text: &str) {
  use std::io::Write as _;
  let mut out = std::io::stdout();
  if out.write_all(text.as_bytes()).is_ok() && out.flush().is_ok() {
    return;
  }
  #[cfg(windows)]
  {
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    unsafe {
      windows_sys::Win32::System::Console::AttachConsole(ATTACH_PARENT_PROCESS);
    }
    if let Ok(mut console) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
      let _ = console.write_all(text.as_bytes());
    }
  }
  #[cfg(not(windows))]
  {
    let _ = out.write_all(text.as_bytes());
  }
}

/// Show a message box for failures a double-clicked launch would otherwise
/// swallow (there is no console to print them to).
#[cfg(windows)]
fn alert(text: &str) {
  use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
  let mut text: Vec<u16> = text.encode_utf16().collect();
  let mut caption: Vec<u16> = "Wish".encode_utf16().collect();
  text.push(0);
  caption.push(0);
  unsafe {
    MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), MB_OK | MB_ICONERROR);
  }
}
#[cfg(not(windows))]
fn alert(_text: &str) {}

async fn wait_for_signal() {
  #[cfg(unix)]
  {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
      .expect("install SIGTERM handler");
    tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
  }
  #[cfg(windows)]
  {
    let mut ctrl_break = tokio::signal::windows::ctrl_break().expect("install Ctrl-Break handler");
    tokio::select! {
      _ = tokio::signal::ctrl_c() => {},
      _ = ctrl_break.recv() => {},
      _ = console::wait_for_close() => {},
    }
  }
}
#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  // The Windows location is unchanged from v0.3.x: an existing
  // Documents\Wish config and its data must keep being found.
  #[test]
  fn windows_user_dir_is_documents_wish() {
    let dir = user_dir_for(Platform::Windows, Some(OsStr::new(r"C:\Users\Ada")), None).unwrap();
    assert_eq!(dir, PathBuf::from(r"C:\Users\Ada").join("Documents").join("Wish"));
  }

  #[test]
  fn macos_user_dir_is_application_support() {
    let dir = user_dir_for(Platform::MacOs, Some(OsStr::new("/Users/ada")), None).unwrap();
    assert_eq!(
      dir,
      PathBuf::from("/Users/ada").join("Library").join("Application Support").join("Wish")
    );
  }

  #[test]
  fn linux_user_dir_prefers_absolute_xdg_config_home() {
    let dir =
      user_dir_for(Platform::Linux, Some(OsStr::new("/home/ada")), Some(OsStr::new("/srv/cfg")))
        .unwrap();
    assert_eq!(dir, PathBuf::from("/srv/cfg").join("wish"));
  }

  #[test]
  fn linux_user_dir_ignores_relative_xdg_config_home() {
    let dir =
      user_dir_for(Platform::Linux, Some(OsStr::new("/home/ada")), Some(OsStr::new("wish-cfg")))
        .unwrap();
    assert_eq!(dir, PathBuf::from("/home/ada").join(".config").join("wish"));
  }

  #[test]
  fn linux_user_dir_falls_back_to_home() {
    let dir = user_dir_for(Platform::Linux, Some(OsStr::new("/home/ada")), None).unwrap();
    assert_eq!(dir, PathBuf::from("/home/ada").join(".config").join("wish"));
  }

  // Without a home there is nowhere to put user data — and a first run must
  // fail loudly instead of writing beside the executable.
  #[test]
  fn user_dir_needs_a_home_except_for_absolute_xdg() {
    assert!(user_dir_for(Platform::Windows, None, None).is_none());
    assert!(user_dir_for(Platform::MacOs, None, None).is_none());
    assert!(user_dir_for(Platform::Linux, None, None).is_none());
    assert!(user_dir_for(Platform::Linux, None, Some(OsStr::new("/srv/cfg"))).is_some());
  }

  // data_dir is derived from the user directory, so sessions, attachments and
  // the window profile never land in the install directory.
  #[test]
  fn default_config_keeps_data_inside_the_user_dir() {
    let dir = PathBuf::from("/Users/ada/Library/Application Support/Wish");
    let config = default_config(&dir, None);
    assert_eq!(config["data_dir"], json!(dir.join("data")));
    assert_eq!(config["listen"], json!("127.0.0.1:8787"));
    assert!(
      config.get("shell").is_none(),
      "no niubash beside the executable means no shell override"
    );
  }

  #[test]
  fn default_config_wires_the_bundled_niubash() {
    let dir = PathBuf::from(r"C:\Users\Ada\Documents\Wish");
    let niu = PathBuf::from(r"C:\Program Files\Wish\niubash\niu.exe");
    let config = default_config(&dir, Some(&niu));
    assert_eq!(config["shell"]["program"], json!(niu));
    assert_eq!(config["shell"]["args"], json!(["-c"]));
  }

  // Each platform gets its own launcher; macOS must not fall through to
  // xdg-open, which does not exist there.
  #[test]
  fn browser_command_matches_the_platform() {
    let url = "http://127.0.0.1:8787";
    assert_eq!(
      browser_command(Platform::Windows, url),
      ("cmd", vec!["/C".to_owned(), "start".to_owned(), String::new(), url.to_owned()])
    );
    assert_eq!(browser_command(Platform::MacOs, url), ("open", vec![url.to_owned()]));
    assert_eq!(browser_command(Platform::Linux, url), ("xdg-open", vec![url.to_owned()]));
  }

  #[test]
  fn current_platform_matches_the_build() {
    let platform = current_platform();
    if cfg!(windows) {
      assert_eq!(platform, Platform::Windows);
    } else if cfg!(target_os = "macos") {
      assert_eq!(platform, Platform::MacOs);
    } else {
      assert_eq!(platform, Platform::Linux);
    }
  }
}