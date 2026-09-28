//! HTTP application, provider configuration and operational session management.

mod app;
mod catalog;
mod codex_login;
mod config;
mod configuration;
mod error;
mod http;
mod management;
mod media;
mod presets;
mod provider;
mod sampling;
mod session;
#[cfg(windows)]
mod shell_window;
mod skills;
mod subagents;

use std::net::SocketAddr;
use std::path::PathBuf;
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
  let mut path: Option<PathBuf> = None;
  // A bare double-click opens the application window; any argument means an operator is present.
  let mut shell = if args.is_empty() { Shell::Window } else { Shell::Headless };
  let mut version = false;
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
      "--version" | "-V" => version = true,
      "--help" | "-h" => {
        cli_out(
          "wish [--config <config.json>] [--no-window] [--window] [--browser]
One HTTP service for providers and sessions, with a web interface.

Without --config, wish looks for config.json beside the executable, then in the
user's Documents\\Wish folder, generating a default one there on first run.
Without other flags wish opens its interface in an application window;
--no-window serves headless and --browser uses the system browser instead.
",
        );
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
  let (app, listener) = rt.block_on(async {
    let config: config::Config = serde_json::from_slice(&tokio::fs::read(&config_path).await?)?;
    let app = app::App::open(&config, config_path.clone()).await?;
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    Ok::<_, Box<dyn std::error::Error>>((app, listener))
  })?;
  let addr = listener.local_addr()?;
  match shell {
    Shell::Headless => {
      cli_out(&format!("wish listening on http://{addr}\n"));
      rt.block_on(serve(app, listener))?;
    }
    Shell::Window | Shell::Browser => {
      let server = std::thread::spawn(move || {
        let _ = rt.block_on(serve(app, listener));
      });
      if matches!(shell, Shell::Window) {
        let profile = config_path.parent().map(|dir| dir.join("window-profile"));
        #[cfg(windows)]
        {
          if let Some(dir) = &profile {
            let _ = std::fs::create_dir_all(dir);
          }
          match shell_window::run(addr, profile) {
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

/// Serve HTTP until the shutdown signal; the signal (Ctrl-C where a console
/// exists) drains sessions before the process exits.
async fn serve(app: Arc<app::App>, listener: tokio::net::TcpListener) -> Result<(), Box<dyn std::error::Error>> {
  let shutdown = app.clone();
  let result = axum::serve(listener, http::build_router(app.clone()))
    .with_graceful_shutdown(async move {
      wait_for_signal().await;
      shutdown.begin_shutdown().await;
    })
    .await;
  app.begin_shutdown().await;
  app.finish_shutdown().await?;
  result?;
  Ok(())
}

/// Locate a configuration for double-click launches: beside the executable first,
/// then the user's Documents\\Wish folder, generating a default one there on first run.
async fn auto_config() -> Result<(PathBuf, bool), Box<dyn std::error::Error>> {
  let exe = std::env::current_exe()?;
  if let Some(dir) = exe.parent() {
    let beside = dir.join("config.json");
    if beside.exists() {
      return Ok((beside, false));
    }
  }
  let Some(home) = std::env::var_os("USERPROFILE") else {
    return Err("no --config given and no user profile found".into());
  };
  let dir = PathBuf::from(home).join("Documents").join("Wish");
  let path = dir.join("config.json");
  if path.exists() {
    return Ok((path, false));
  }
  tokio::fs::create_dir_all(&dir).await?;
  let mut json = serde_json::json!({
    "listen": "127.0.0.1:8787",
    "data_dir": dir.join("data"),
  });
  // Sessions shell out through the bundled niubash when the installer placed it beside the executable.
  if let Some(niu) = exe.parent().map(|p| p.join("niubash").join("niu.exe")) {
    if niu.exists() {
      json["shell"] = serde_json::json!({ "program": niu, "args": ["-c"] });
    }
  }
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

/// Open the system's default browser on the listening address (best effort).
fn open_system_browser(addr: SocketAddr) {
  let url = host_url(addr);
  #[cfg(windows)]
  let spawned = std::process::Command::new("cmd").args(["/C", "start", "", &url]).spawn();
  #[cfg(not(windows))]
  let spawned = std::process::Command::new("xdg-open").arg(&url).spawn();
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
  #[cfg(not(unix))]
  {
    tokio::signal::ctrl_c().await.expect("install Ctrl-C handler");
  }
}
