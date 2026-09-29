mod ask;
pub(crate) mod content;
mod directories;
mod manage;
mod providers;
mod sessions;
mod statistics;
mod skills;
mod workspace;
use crate::server::{app::App, error::ApiError};
use axum::{
  Json, Router,
  body::Body,
  extract::{Request, State},
  http::{HeaderValue, Method, StatusCode, header},
  middleware::{self, Next},
  response::{IntoResponse, Response},
  routing::{get, post, put},
};
use serde_json::json;
use std::sync::Arc;

pub fn build_router(app: Arc<App>) -> Router {
  let api = Router::new()
    .route("/status", get(statistics::status))
    .route("/storage", get(statistics::storage))
    .route("/usage", get(statistics::global_usage))
    .route("/usage/series", get(statistics::global_series))
    .route("/usage/daily", get(statistics::global_daily))
    .route("/sessions/{id}/usage", get(statistics::session_usage))
    .route("/sessions/{id}/usage/series", get(statistics::session_series))
    .route("/sessions/{id}/usage/daily", get(statistics::session_daily))
    .route("/sessions/{id}/ask", post(ask::ask))
    .route("/sessions/{id}/input", post(content::input))
    .route("/sessions/{id}/blobs", post(content::upload))
    .route("/sessions/{id}/blobs/{blob}", get(content::download))
    .route("/sessions/{id}/blobs/{blob}/meta", get(content::metadata))
    .route("/sessions/{id}/history", get(content::timeline))
    .route("/sessions/{id}/workspace", get(workspace::list))
    .route("/config", get(manage::configuration).put(manage::save_configuration))
    .route("/proxy-environment", get(|| async { Json(crate::server::config::proxy_environment()) }))
    .route("/shells", get(|| async { Json(crate::server::config::shell_catalog()) }))
    .route("/defaults", get(manage::defaults))
    .route("/directories", get(directories::list))
    .route("/skills", get(skills::catalog))
    .route("/events", get(manage::events))
    .route(
      "/version",
      get(|| async { Json(json!({"name":"wish","version":env!("CARGO_PKG_VERSION")})) }),
    )
    .route("/sessions/{id}/context/clear", post(manage::clear_context))
    .route("/sessions/{id}/rewind", post(manage::rewind))
    .route("/sessions/{id}/fork", post(manage::fork))
    .route(
      "/sessions/{id}/queue/{entry}",
      axum::routing::delete(manage::cancel_input).patch(manage::move_input),
    )
    .route("/provider-presets", get(|| async { Json(crate::server::presets::catalog()) }))
    .route("/providers", get(providers::list))
    .route("/providers/{id}", get(providers::get))
    .route("/providers/{id}/models", get(providers::models))
    .route("/providers/models/probe", post(providers::probe_models))
    .route("/providers/{id}/account", get(providers::account))
    .route(
      "/providers/{id}/chatgpt-login",
      post(crate::server::codex_login::start).get(crate::server::codex_login::status),
    )
    .route(
      "/providers/{id}/chatgpt-login/complete",
      post(crate::server::codex_login::complete),
    )
    .route("/providers/{id}/call", post(providers::call))
    .route("/providers/{id}/count-tokens", post(providers::count))
    .route("/providers/{id}/compact", post(providers::compact))
    .route("/sessions", get(sessions::list).post(sessions::create))
    .route(
      "/sessions/{id}",
      get(sessions::get).patch(manage::update_session).delete(manage::delete_session),
    )
    .route("/sessions/{id}/messages", post(sessions::enqueue))
    .route("/sessions/{id}/config", put(sessions::set_config))
    .route("/sessions/{id}/metadata", put(sessions::set_metadata))
    .route("/sessions/{id}/shell", put(sessions::set_shell))
    .route("/sessions/{id}/run", post(sessions::run))
    .route("/sessions/{id}/interrupt", post(sessions::interrupt))
    .route("/sessions/{id}/compact", post(sessions::compact))
    .route("/sessions/{id}/events", get(sessions::events))
    .route("/sessions/{id}/entries", get(sessions::entries))
    .route("/sessions/{id}/queue", get(sessions::queue))
    .route("/sessions/{id}/generations", get(sessions::generations))
    .route("/sessions/{id}/generations/{generation}/entries", get(sessions::generation_entries))
    .route("/sessions/{id}/calls", get(sessions::calls))
    .route("/sessions/{id}/history/query", post(sessions::query_history))
    .route("/sessions/{id}/history/search", post(sessions::search_history))
    .route("/sessions/{id}/history/{sequence}", get(sessions::read_history))
    .layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024))
    .route_layer(middleware::from_fn_with_state(app.clone(), authorize));
  Router::new()
    .nest("/api", api)
    .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
    .route(
      "/version",
      get(|| async { Json(json!({"name":"wish","version":env!("CARGO_PKG_VERSION")})) }),
    )
    .fallback(static_or_spa)
    .layer(middleware::from_fn_with_state(app.clone(), cross_origin))
    .with_state(app)
}
/// Serve the bundled web interface: real files from the directory beside the
/// executable, with index.html answering client-side routes, while /api keeps
/// its JSON 404.
async fn static_or_spa(State(app): State<Arc<App>>, request: Request) -> Response {
  let Some(dir) = app.web_dir.clone() else {
    return ApiError::not_found().into_response();
  };
  let method = request.method();
  if method != Method::GET && method != Method::HEAD {
    return ApiError::not_found().into_response();
  }
  let path = request.uri().path().to_owned();
  if path == "/api" || path.starts_with("/api/") {
    return ApiError::not_found().into_response();
  }
  let rel = percent_decode(&path).trim_start_matches('/').to_owned();
  // Anything that could escape the web directory: dotdot climbs, drive letters
  // and leading separators make PathBuf::join replace its base, NUL confuses the
  // filesystem. Absolute paths never reach a real file this way.
  let escapes = rel.starts_with('\\')
    || rel.split(['/', '\\']).any(|part| part == ".." || part.contains(':') || part.contains('\0'));
  let (file, index) = if rel.is_empty() || escapes {
    (dir.join("index.html"), true)
  } else {
    let candidate = dir.join(&rel);
    match tokio::fs::metadata(&candidate).await {
      Ok(meta) if meta.is_file() => (candidate, false),
      _ => (dir.join("index.html"), true),
    }
  };
  let bytes = match tokio::fs::read(&file).await {
    Ok(bytes) => bytes,
    Err(_) => return ApiError::not_found().into_response(),
  };
  let mut response = Response::new(Body::from(bytes));
  let headers = response.headers_mut();
  headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(&file, index)));
  let cache = if index || !rel.starts_with("assets/") {
    "no-cache"
  } else {
    "public, max-age=31536000, immutable"
  };
  headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
  response
}

fn content_type(file: &std::path::Path, index: bool) -> &'static str {
  if index {
    return "text/html; charset=utf-8";
  }
  match file
    .extension()
    .and_then(|extension| extension.to_str())
    .map(str::to_ascii_lowercase)
    .as_deref()
  {
    Some("html" | "htm") => "text/html; charset=utf-8",
    Some("js" | "mjs") => "text/javascript; charset=utf-8",
    Some("css") => "text/css; charset=utf-8",
    Some("json" | "map") => "application/json; charset=utf-8",
    Some("webmanifest") => "application/manifest+json; charset=utf-8",
    Some("svg") => "image/svg+xml",
    Some("png") => "image/png",
    Some("jpg" | "jpeg") => "image/jpeg",
    Some("gif") => "image/gif",
    Some("webp") => "image/webp",
    Some("avif") => "image/avif",
    Some("ico") => "image/x-icon",
    Some("woff") => "font/woff",
    Some("woff2") => "font/woff2",
    Some("ttf") => "font/ttf",
    Some("otf") => "font/otf",
    Some("txt" | "md") => "text/plain; charset=utf-8",
    Some("wasm") => "application/wasm",
    _ => "application/octet-stream",
  }
}

/// Decode percent escapes in a URL path, byte by byte.
fn percent_decode(input: &str) -> String {
  let bytes = input.as_bytes();
  let mut out = Vec::with_capacity(bytes.len());
  let hex = |byte: u8| match byte {
    b'0'..=b'9' => byte - b'0',
    b'a'..=b'f' => byte - b'a' + 10,
    b'A'..=b'F' => byte - b'A' + 10,
    _ => 0xff,
  };
  let mut i = 0;
  while i < bytes.len() {
    if bytes[i] == b'%' && i + 2 < bytes.len() {
      let (high, low) = (hex(bytes[i + 1]), hex(bytes[i + 2]));
      if high != 0xff && low != 0xff {
        out.push(high << 4 | low);
        i += 3;
        continue;
      }
    }
    out.push(bytes[i]);
    i += 1;
  }
  String::from_utf8_lossy(&out).into_owned()
}
/// Pages from other origins may call a server that requires a token: they cannot
/// act without knowing it. A server without one answers its own origin only, so
/// an arbitrary web page cannot drive a local agent through the visitor's browser.
async fn cross_origin(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
  if app.token.is_none() || !request.headers().contains_key(header::ORIGIN) {
    return next.run(request).await;
  }
  let preflight = request.method() == Method::OPTIONS
    && request.headers().contains_key(header::ACCESS_CONTROL_REQUEST_METHOD);
  let private_network = request.headers().contains_key("access-control-request-private-network");
  let mut response =
    if preflight { StatusCode::NO_CONTENT.into_response() } else { next.run(request).await };
  let headers = response.headers_mut();
  headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
  if preflight {
    headers.insert(
      header::ACCESS_CONTROL_ALLOW_METHODS,
      HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE"),
    );
    headers.insert(
      header::ACCESS_CONTROL_ALLOW_HEADERS,
      HeaderValue::from_static("authorization, content-type, if-match, last-event-id"),
    );
    headers.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
    // Chrome asks before a public page reaches a server on a private address.
    if private_network {
      headers.insert("access-control-allow-private-network", HeaderValue::from_static("true"));
    }
  }
  response
}
async fn authorize(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
  if let Some(token) = &app.token {
    let supplied = request
      .headers()
      .get("authorization")
      .and_then(|h| h.to_str().ok())
      .and_then(|h| h.strip_prefix("Bearer "));
    if supplied != Some(token.as_str()) {
      return (StatusCode::UNAUTHORIZED, Json(json!({"error":{"message":"unauthorized"}})))
        .into_response();
    }
  }
  next.run(request).await
}
