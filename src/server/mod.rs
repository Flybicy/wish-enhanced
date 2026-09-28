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
mod skills;
mod subagents;

pub(crate) async fn run() -> Result<(), Box<dyn std::error::Error>> {
  let args: Vec<String> = std::env::args().skip(1).collect();
  let mut path: Option<std::path::PathBuf> = None;
  let mut open_browser = args.is_empty();
  let mut version = false;
  let mut i = 0;
  while i < args.len() {
    match args[i].as_str() {
      "--config" => {
        i += 1;
        path = Some(std::path::PathBuf::from(
          args.get(i).ok_or("usage: wish [--config <config.json>] [--no-browser]")?,
        ));
        // Explicit configurations serve headless use; open the browser only on request.
        open_browser = false;
      }
      "--no-browser" => open_browser = false,
      "--browser" => open_browser = true,
      "--version" | "-V" => version = true,
      "--help" | "-h" => {
        println!(
          "wish [--config <config.json>] [--no-browser]
One HTTP service for providers and sessions.

Without --config, wish looks for config.json beside the executable, then in the
user's Documents\\Wish folder, generating a default one there on first run and
opening the web interface in the system browser."
        );
        return Ok(());
      }
      other => return Err(format!("unknown argument: {other}").into()),
    }
    i += 1;
  }
  if version {
    println!("wish {}", env!("CARGO_PKG_VERSION"));
    return Ok(());
  }
  let (path, generated) = match path {
    Some(path) => (path, false),
    None => auto_config().await?,
  };
  if generated {
    eprintln!("wish generated a default config at {}", path.display());
  }
  let config: config::Config = serde_json::from_slice(&tokio::fs::read(&path).await?)?;
  let app = app::App::open(&config, path.into()).await?;
  let listener = tokio::net::TcpListener::bind(config.listen).await?;
  eprintln!("wish listening on {}", listener.local_addr()?);
  if open_browser {
    spawn_browser_opener(listener.local_addr()?);
  }
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
/// then the user's Documents\Wish folder, generating a default one there on first run.
async fn auto_config() -> Result<(std::path::PathBuf, bool), Box<dyn std::error::Error>> {
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
  let dir = std::path::PathBuf::from(home).join("Documents").join("Wish");
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

/// Open the system browser on the listening address (best effort, after a short settle delay).
fn spawn_browser_opener(addr: std::net::SocketAddr) {
  let host = match addr.ip() {
    std::net::IpAddr::V4(v4) if v4.is_unspecified() => "127.0.0.1".to_string(),
    std::net::IpAddr::V6(v6) if v6.is_unspecified() => "[::1]".to_string(),
    ip => ip.to_string(),
  };
  let url = format!("http://{host}:{}", addr.port());
  tokio::spawn(async move {
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    #[cfg(windows)]
    let spawned =
      std::process::Command::new("cmd").args(["/C", "start", "", &url]).spawn();
    #[cfg(not(windows))]
    let spawned = std::process::Command::new("xdg-open").arg(&url).spawn();
    if let Err(error) = spawned {
      eprintln!("wish could not open a browser ({error}); visit {url} manually");
    }
  });
}

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
