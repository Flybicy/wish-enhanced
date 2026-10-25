//! What a browser meets besides the API: the web app, when one is installed, and the checks that
//! keep other web pages from driving a server that asks for no token.
//!
//! A server without a token answers only requests addressed to a name it knows - loopback, the
//! address it listens on, and `allowed_hosts` - so a page whose domain is later pointed at this
//! machine (DNS rebinding) is refused, and changes must come from the server's own origin. With a
//! token neither check applies: nothing is done without it.
use crate::server::error::ApiError;
use axum::{
  body::Body,
  extract::{Request, State},
  http::{Method, StatusCode, header},
  middleware::Next,
  response::{IntoResponse, Response},
};
use std::{
  collections::HashSet,
  net::SocketAddr,
  path::{Path, PathBuf},
  sync::Arc,
};

pub struct WebFront {
  /// The built web app, canonical, when one is installed.
  app_dir: Option<PathBuf>,
  /// The `host[:port]` names a browser may address this server by; `None` when it requires a token.
  hosts: Option<HashSet<String>>,
  /// The token this server asks for, written into the page it serves so the app
  /// can send it; `None` when the server takes writes from its own origin only.
  token: Option<String>,
}

impl WebFront {
  pub fn new(
    app_dir: Option<PathBuf>,
    listen: SocketAddr,
    allowed_hosts: &[String],
    token: Option<String>,
  ) -> Self {
    let hosts = token.is_none().then(|| known_hosts(listen, allowed_hosts));
    Self { app_dir, hosts, token }
  }
  pub fn app_dir(&self) -> Option<&Path> {
    self.app_dir.as_deref()
  }
  /// The token the page this server serves carries.
  pub fn bearer_token(&self) -> Option<&str> {
    self.token.as_deref()
  }
}

/// The web app's directory: the configured one, or `web` beside the program, where every package
/// and release archive puts it. A configured directory without an `index.html` is a mistake worth
/// stopping for; finding none beside the program only means this server answers the API alone.
pub fn find_app_dir(configured: Option<&Path>) -> Result<Option<PathBuf>, String> {
  let dir = match configured {
    Some(dir) => dir.to_owned(),
    // Canonical, so a link to the program, such as `wish` on the PATH, finds the real one.
    None => match std::env::current_exe().and_then(|program| program.canonicalize()) {
      Ok(program) => program.with_file_name("web"),
      Err(_) => return Ok(None),
    },
  };
  if !dir.join("index.html").is_file() {
    return match configured {
      Some(_) => Err(format!("web_dir {} holds no index.html", dir.display())),
      None => Ok(None),
    };
  }
  dir.canonicalize().map(Some).map_err(|error| format!("web app {}: {error}", dir.display()))
}

/// Loopback names on the server's port, the address it listens on unless that is every address,
/// and the ones the configuration allows.
fn known_hosts(listen: SocketAddr, allowed_hosts: &[String]) -> HashSet<String> {
  let port = listen.port();
  let loopback = ["127.0.0.1", "localhost", "[::1]"];
  let mut hosts: HashSet<String> = loopback.iter().map(|name| format!("{name}:{port}")).collect();
  if port == 80 {
    hosts.extend(loopback.map(str::to_owned));
  }
  if !listen.ip().is_unspecified() {
    hosts.insert(listen.to_string());
  }
  hosts.extend(
    allowed_hosts
      .iter()
      .map(|host| host.trim().to_ascii_lowercase())
      .filter(|host| !host.is_empty()),
  );
  hosts
}

/// Refuses, on a server without a token, requests addressed to an unknown name and changes from
/// another origin.
pub async fn guard(State(front): State<Arc<WebFront>>, request: Request, next: Next) -> Response {
  let Some(hosts) = &front.hosts else { return next.run(request).await };
  let headers = request.headers();
  let text = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
  let host = text(header::HOST.as_str()).unwrap_or_default().to_ascii_lowercase();
  if !hosts.contains(&host) {
    return refuse(StatusCode::MISDIRECTED_REQUEST, "unexpected host");
  }
  let changes =
    matches!(*request.method(), Method::POST | Method::PUT | Method::PATCH | Method::DELETE);
  if changes {
    if let Some(origin) = text(header::ORIGIN.as_str())
      && origin != format!("http://{host}")
      && origin != format!("https://{host}")
    {
      return refuse(StatusCode::FORBIDDEN, "cross-origin request");
    }
    if let Some(site) = text("sec-fetch-site")
      && site != "same-origin"
      && site != "none"
    {
      return refuse(StatusCode::FORBIDDEN, "cross-site request");
    }
  }
  next.run(request).await
}

fn refuse(status: StatusCode, message: &str) -> Response {
  ApiError { status, message: message.to_owned(), details: None }.into_response()
}

/// A file of the web app, or its page for a path that names no file. Anything under `/api` that no
/// route answered stays the API's 404.
pub async fn serve(front: &WebFront, request: Request) -> Response {
  let path = request.uri().path();
  let not_found = || ApiError::not_found().into_response();
  let Some(root) = front.app_dir() else { return not_found() };
  if path == "/api" || path.starts_with("/api/") {
    return not_found();
  }
  if !matches!(*request.method(), Method::GET | Method::HEAD) {
    return StatusCode::METHOD_NOT_ALLOWED.into_response();
  }
  let segments: Vec<&str> = path.split('/').filter(|segment| !segment.is_empty()).collect();
  // Vite names its files plainly; anything that could climb out of the directory is no file of it.
  if segments.iter().any(|segment| segment.starts_with('.') || segment.contains(['%', '\\', '\0']))
  {
    return not_found();
  }
  let named = segments.iter().fold(root.to_owned(), |path, segment| path.join(segment));
  let file = match tokio::fs::canonicalize(&named).await {
    Ok(file) if file.starts_with(root) && file.is_file() => file,
    // The app routes in its URL's fragment, so a path without a file extension is its page.
    _ if segments.last().is_none_or(|last| !last.contains('.')) => root.join("index.html"),
    _ => return not_found(),
  };
  let Ok(bytes) = tokio::fs::read(&file).await else { return not_found() };
  let page = file.file_name().is_some_and(|name| name == "index.html");
  let bytes = if page { page_with_token(bytes, front.bearer_token()) } else { bytes };
  let cache =
    if path.starts_with("/assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
  let body = if request.method() == Method::HEAD { Body::empty() } else { Body::from(bytes) };
  (
    [
      (header::CONTENT_TYPE, content_type(&file)),
      (header::CACHE_CONTROL, cache),
      (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    ],
    body,
  )
    .into_response()
}

/// The web app's page with the token this server asks for in its markup, which
/// the app reads and sends on every write. A page on another origin cannot read
/// it: `cross_origin` answers no such headers for the app's own files.
fn page_with_token(bytes: Vec<u8>, token: Option<&str>) -> Vec<u8> {
  let Some(token) = token else { return bytes };
  let Ok(text) = std::str::from_utf8(&bytes) else { return bytes };
  let meta = format!(r#"<meta name="wish-bearer-token" content="{}" />"#, escaped(token));
  // After `<head>` when the page has one, before everything else when it does not.
  let lower = text.to_ascii_lowercase();
  let head =
    lower.find("<head").and_then(|start| lower[start..].find('>').map(|end| start + end + 1));
  let page = match head {
    Some(at) => format!("{}{meta}{}", &text[..at], &text[at..]),
    None => format!("{meta}{text}"),
  };
  page.into_bytes()
}

/// A token for an attribute value, where quotes and ampersands mean something
/// and a configuration may hold anything.
fn escaped(token: &str) -> String {
  token
    .chars()
    .fold(String::with_capacity(token.len()), |mut out, character| {
      match character {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        '\'' => out.push_str("&#39;"),
        _ => out.push(character),
      }
      out
    })
}

fn content_type(file: &Path) -> &'static str {
  match file.extension().and_then(|extension| extension.to_str()).unwrap_or_default() {
    "html" => "text/html; charset=utf-8",
    "js" | "mjs" => "text/javascript; charset=utf-8",
    "css" => "text/css; charset=utf-8",
    "json" | "map" => "application/json",
    "webmanifest" => "application/manifest+json",
    "svg" => "image/svg+xml",
    "png" => "image/png",
    "jpg" | "jpeg" => "image/jpeg",
    "webp" => "image/webp",
    "ico" => "image/x-icon",
    "woff2" => "font/woff2",
    "woff" => "font/woff",
    "wasm" => "application/wasm",
    "txt" => "text/plain; charset=utf-8",
    _ => "application/octet-stream",
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const PAGE: &str = r#"<!doctype html>
<html lang="zh">
  <head>
    <meta charset="UTF-8" />
    <title>Wish</title>
  </head>
  <body><div id="app"></div></body>
</html>
"#;

  // The page the server hands out carries the token, right after `<head>`, so
  // the app reads it before anything else runs.
  #[test]
  fn the_page_carries_the_token_it_asks_for() {
    let page = page_with_token(PAGE.as_bytes().to_vec(), Some("0123456789abcdef"));
    let page = String::from_utf8(page).unwrap();
    let meta = r#"<meta name="wish-bearer-token" content="0123456789abcdef" />"#;
    let head = PAGE.find("<head>").unwrap() + "<head>".len();
    assert!(page.contains(meta), "{page}");
    assert_eq!(page.find(meta).unwrap(), head, "the token goes after <head>");
    assert!(page.contains(r#"<title>Wish</title>"#), "the rest of the page stays");
  }

  // A server that asks for no token serves its page untouched, and a page with
  // no `<head>` still gets the token ahead of everything else.
  #[test]
  fn no_token_leaves_the_page_alone() {
    assert_eq!(page_with_token(PAGE.as_bytes().to_vec(), None), PAGE.as_bytes());
    let page = page_with_token(b"<html><body>hi</body></html>".to_vec(), Some("t"));
    assert_eq!(
      String::from_utf8(page).unwrap(),
      r#"<meta name="wish-bearer-token" content="t" /><html><body>hi</body></html>"#
    );
  }

  // A token is configuration, so it is escaped for an attribute value rather
  // than written into markup raw.
  #[test]
  fn a_token_is_escaped_for_markup() {
    let page = page_with_token(PAGE.as_bytes().to_vec(), Some(r#"a"&<>'"#));
    let page = String::from_utf8(page).unwrap();
    assert!(page.contains(r#"content="a&quot;&amp;&lt;&gt;&#39;"#), "{page}");
  }
}
