//! The HTTP API: the routes, and the layers every request passes - the checks a server without a
//! token makes (see [`web`]), cross-origin rules and the bearer token. Everything lives under
//! `/api` except the unauthenticated `/health` and `/version`; other paths are the web app's.
mod ask;
mod config;
pub(crate) mod content;
mod directories;
mod folders;
mod groups;
mod history;
mod manage;
mod mcp;
mod peers;
mod providers;
mod search;
mod sessions;
mod sse;
mod status;
mod storage;
mod usage;
use crate::server::{
  app::App,
  codex_login, copilot_login, presets,
  web::{self, WebFront},
};
mod skills;
mod workspace;

use axum::{
  Json, Router,
  body::Body,
  extract::{Request, State},
  http::{HeaderValue, Method, StatusCode, header},
  middleware::{self, Next},
  response::{IntoResponse, Response},
  routing::{delete, get, patch, post, put},
};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn build_router(app: Arc<App>, front: Arc<WebFront>) -> Router {
  let api = Router::new()
    .route("/status", get(status::status))
    .route("/events", get(status::events))
    .route("/version", get(version))
    .route("/storage", get(storage::storage))
    .route("/storage/sessions", get(storage::session_storage))
    .route("/storage/prune", post(storage::prune))
    .route("/storage/detail", get(storage::detail))
    .route("/storage/prune-usage", post(storage::prune_usage))
    .route("/usage", get(usage::global_usage))
    .route("/usage/series", get(usage::global_series))
    .route("/usage/daily", get(usage::global_daily))
    .route("/sessions/{id}/usage", get(usage::session_usage))
    .route("/sessions/{id}/usage/series", get(usage::session_series))
    .route("/sessions/{id}/usage/daily", get(usage::session_daily))
    .route("/config", get(config::get).put(config::save))
    .route("/proxy-environment", get(config::proxy_environment))
    .route("/shells", get(config::shells))
    .route("/defaults", get(config::defaults))
    .route("/directories", get(directories::list))
    .route("/provider-presets", get(|| async { Json(presets::provider_presets()) }))
    .route("/search-presets", get(|| async { Json(presets::search_presets()) }))
    .route("/search/providers", get(search::list))
    .route("/search/providers/{id}/check", post(search::check))

    .route("/providers", get(providers::list))
    .route("/provider-draft/models", post(providers::draft_models))
    .route("/providers/{id}", get(providers::get))
    .route("/providers/{id}/models", get(providers::models))
    .route("/providers/models/probe", post(providers::probe_models))
    .route("/providers/{id}/account", get(providers::account))
    .route("/providers/{id}/chatgpt-login", post(codex_login::start).get(codex_login::status))
    .route("/providers/{id}/chatgpt-login/complete", post(codex_login::complete))
    .route("/providers/{id}/copilot-login", post(copilot_login::start).get(copilot_login::status))
    .route("/providers/{id}/call", post(providers::call))
    .route("/providers/{id}/count-tokens", post(providers::count))
    .route("/providers/{id}/compact", post(providers::compact))
    .route("/sessions", get(sessions::list).post(sessions::create))
    .route(
      "/sessions/{id}",
      get(sessions::get).patch(manage::update_session).delete(manage::delete_session),
    )
    .route("/conversations", get(sessions::conversations))
    .route("/conversations/move", post(folders::move_entries))
    .route("/conversations/pin", post(folders::pin))
    .route("/folders", get(folders::list).post(folders::create))
    .route("/folders/{id}", patch(folders::rename).delete(folders::delete))
    .route("/groups", post(groups::create))
    .route("/groups/{id}", get(groups::get).patch(groups::update).delete(groups::delete))
    .route("/groups/{id}/messages", get(groups::messages).post(groups::post))
    .route("/groups/{id}/blobs", post(content::upload))
    .route("/groups/{id}/blobs/{blob}", get(content::download))
    .route("/groups/{id}/blobs/{blob}/meta", get(content::metadata))
    .route("/sessions/{id}/groups", get(groups::of_session))
    .route("/sessions/{id}/fork", post(manage::fork))
    .route("/sessions/{id}/rewind", post(manage::rewind))
    .route("/sessions/{id}/context/clear", post(manage::clear_context))
    .route("/sessions/{id}/messages", post(sessions::enqueue))
    .route("/sessions/{id}/input", post(content::input))
    .route("/sessions/{id}/queue/{entry}", delete(manage::cancel_input).patch(manage::move_input))
    .route("/sessions/{id}/blobs", post(content::upload))
    .route("/sessions/{id}/blobs/{blob}", get(content::download))
    .route("/sessions/{id}/blobs/{blob}/meta", get(content::metadata))
    .route("/sessions/{id}/config", put(sessions::set_config))
    .route("/sessions/{id}/permission", post(sessions::set_permission))
    .route("/sessions/{id}/approvals/{call_id}", post(sessions::resolve_approval))
    .route("/sessions/{id}/goal", post(sessions::set_goal))
    .route("/sessions/{id}/metadata", put(sessions::set_metadata))
    .route("/sessions/{id}/shell", put(sessions::set_shell))
    .route("/sessions/{id}/tools", put(sessions::set_tools))
    .route("/sessions/{id}/answer", post(sessions::answer))
    .route("/sessions/{id}/ask", post(ask::ask))
    .route("/sessions/{id}/run", post(sessions::run))
    .route("/sessions/{id}/interrupt", post(sessions::interrupt))
    .route("/sessions/{id}/compact", post(sessions::compact))
    .route("/sessions/{id}/events", get(sessions::events))
    .route("/sessions/{id}/entries", get(sessions::entries))
    .route("/sessions/{id}/queue", get(sessions::queue))
    .route("/sessions/{id}/generations", get(sessions::generations))
    .route("/sessions/{id}/generations/{generation}/entries", get(sessions::generation_entries))
    .route("/sessions/{id}/calls", get(sessions::calls))
    .route("/sessions/{id}/history", get(history::timeline))
    .route("/sessions/{id}/history/query", post(history::query))
    .route("/sessions/{id}/history/search", post(history::search))
    .route("/sessions/{id}/history/{sequence}", get(history::read))
    .route("/mcp/servers", get(mcp::list))
    .route("/mcp/servers/{id}/check", post(mcp::check))
    .layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024))
    .route_layer(middleware::from_fn_with_state(app.clone(), authorize))
    // The bridge a session's shell reaches its MCP servers through checks that session's own token
    // instead of the application's.
    .route("/sessions/{id}/mcp/servers", get(mcp::servers))
    .route("/sessions/{id}/mcp/tool", get(mcp::tool))
    .route("/sessions/{id}/mcp/call", post(mcp::call))
    .route("/sessions/{id}/skills", get(skills::session_list))
    .route("/sessions/{id}/skills/{name}", get(skills::session_show))
    .route("/sessions/{id}/peers", get(peers::list).post(peers::create))
    .route("/sessions/{id}/peers/groups", post(peers::create_group))
    .route(
      "/sessions/{id}/peers/{target}",
      get(peers::show).patch(peers::configure).delete(peers::delete),
    )
    .route("/sessions/{id}/peers/{target}/send", post(peers::send))
    .route("/sessions/{id}/peers/{target}/members", post(peers::add_members))
    .route("/sessions/{id}/peers/{target}/leave", post(peers::leave))
    .route("/skills/install", post(skills::install))
    .route("/skills", get(skills::list))
    .route("/skills/{name}", get(skills::show));
  Router::new()
    .nest("/api", api)
    .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
    .route("/version", get(version))
    .fallback({
      let front = front.clone();
      move |request: Request| async move { web::serve(&front, request).await }
    })

    .layer(middleware::from_fn_with_state(app.clone(), cross_origin))
    .layer(middleware::from_fn_with_state(front, web::guard))
    .with_state(app)
}
/// The server's identity, for a client checking what it talks to.
async fn version() -> Json<Value> {
  Json(json!({"name":"wish","version":env!("CARGO_PKG_VERSION")}))
}

/// CORS for a token-bearing server: a page on another origin keeps read-only
/// access (`Access-Control-Allow-Origin: *` never authorizes credentials), while
/// every write it attempts is rejected in `authorize` before it reaches a
/// handler. A server without a token answers its own origin only, so an
/// arbitrary web page cannot drive a local agent through the visitor's browser.

async fn cross_origin(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
  if app.bearer_token.is_none() || !request.headers().contains_key(header::ORIGIN) {
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
/// Gate for every `/api` route. With a token configured the bearer must match
/// (compared in constant time); with none configured, reads still answer for
/// diagnostics while every write fails closed — an unauthenticated page in the
/// visitor's browser, or anything else that can reach the port, must not drive
/// the agent.
async fn authorize(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
  let method = request.method().clone();
  let path = request.uri().path().to_owned();
  let (supplied, origin, host, fetch_site) = {
    let headers = request.headers();
    (
      headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(bearer_of),
      headers.get(header::ORIGIN).and_then(|value| value.to_str().ok()),
      headers.get(header::HOST).and_then(|value| value.to_str().ok()),
      headers.get("sec-fetch-site").and_then(|value| value.to_str().ok()),
    )
  };
  match admission(app.bearer_token.as_deref(), &method, supplied, origin, host, fetch_site) {
    Admission::Allow => next.run(request).await,
    Admission::Unauthorized => rejection(
      StatusCode::UNAUTHORIZED,
      "wrong or missing bearer token",
      "unauthorized",
      &method,
      &path,
    ),
    Admission::Unauthenticated => rejection(
      StatusCode::UNAUTHORIZED,
      "write without a bearer token",
      "unauthorized: this server has no bearer token; set bearer_token_env in the configuration and restart to enable write operations",
      &method,
      &path,
    ),
    Admission::CrossOrigin => rejection(
      StatusCode::FORBIDDEN,
      "cross-origin write",
      "forbidden: cross-origin write requests are rejected; call this API from the server's own origin or a CLI",
      &method,
      &path,
    ),
  }
}

/// What `authorize` decides about one request.
#[derive(Debug, PartialEq, Eq)]
enum Admission {
  /// The request reaches its handler.
  Allow,
  /// A token is configured and this request did not carry it.
  Unauthorized,
  /// No token is configured, so a write has nothing to authenticate against.
  Unauthenticated,
  /// A write arrived from another origin.
  CrossOrigin,
}

/// Writes: every method a browser page could use to drive the agent — run,
/// interrupt, permission, approval, shell, metadata, config, input, rewind,
/// fork, delete. GET and HEAD stay open for diagnostics and SSE streams;
/// OPTIONS is the preflight the `cross_origin` layer answers.
fn mutates(method: &Method) -> bool {
  !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// The bearer material of an `Authorization` header value, when it uses the
/// scheme this API documents. The scheme match stays case-sensitive exactly as
/// before, so existing token clients keep working byte for byte.
fn bearer_of(value: &str) -> Option<&str> {
  value.strip_prefix("Bearer ")
}

/// Which requests reach a handler. The cross-origin check runs before the token
/// check because a token in a page's hands must not turn the visitor's browser
/// into a remote control for this agent.
fn admission(
  expected: Option<&str>,
  method: &Method,
  supplied: Option<&str>,
  origin: Option<&str>,
  host: Option<&str>,
  fetch_site: Option<&str>,
) -> Admission {
  if mutates(method) && is_cross_origin(origin, host, fetch_site) {
    return Admission::CrossOrigin;
  }
  match expected {
    Some(expected) => {
      let supplied = supplied.unwrap_or_default();
      if constant_time_eq(supplied.as_bytes(), expected.as_bytes()) {
        Admission::Allow
      } else {
        Admission::Unauthorized
      }
    }
    None => {
      if mutates(method) {
        Admission::Unauthenticated
      } else {
        Admission::Allow
      }
  if let Some(token) = &app.bearer_token {
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
}

/// A cross-origin write, or not. A CLI sends neither header and is not a
/// browser, so it passes; `Sec-Fetch-Site: cross-site` is a forbidden header a
/// page cannot forge, so it alone condemns the request; a present `Origin` is
/// compared against the host the request actually reached.
fn is_cross_origin(origin: Option<&str>, host: Option<&str>, fetch_site: Option<&str>) -> bool {
  if fetch_site.is_some_and(|site| site.eq_ignore_ascii_case("cross-site")) {
    return true;
  }
  let Some(origin) = origin else {
    // No Origin: a CLI, curl, or the SSE reader of a local tool.
    return false;
  };
  match host {
    Some(host) => !origin_is_this_server(origin, host),
    // An Origin without the Host it arrived with cannot be verified; treat it
    // as foreign.
    None => true,
  }
}

/// True when the `Origin` a page sent names this server's own origin. Browsers
/// normalize default ports away, so `http://127.0.0.1` must match a `Host` of
/// `127.0.0.1:80` as well as `127.0.0.1`.
fn origin_is_this_server(origin: &str, host: &str) -> bool {
  let Some((scheme, rest)) = origin.split_once("://") else {
    return false;
  };
  let scheme = scheme.to_ascii_lowercase();
  if scheme != "http" && scheme != "https" {
    return false;
  }
  let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
  let (origin_host, origin_port) = split_host_port(authority);
  let (host_host, host_port) = split_host_port(host);
  if !origin_host.eq_ignore_ascii_case(host_host) {
    return false;
  }
  match (origin_port, host_port) {
    (Some(left), Some(right)) => left == right,
    (None, None) => true,
    (Some(port), None) | (None, Some(port)) => Some(port) == Some(default_port(&scheme)),
  }
}

/// The port a browser drops from an origin because the scheme implies it.
fn default_port(scheme: &str) -> &'static str {
  match scheme {
    "https" => "443",
    _ => "80",
  }
}

/// Splits `host:port`, bracketed IPv6 literals included. A colon followed by
/// anything but digits (or by nothing) is part of the host, not a port.
fn split_host_port(authority: &str) -> (&str, Option<&str>) {
  if let Some(rest) = authority.strip_prefix('[') {
    return match rest.find(']') {
      Some(end) => {
        let after = &rest[end + 1..];
        (&authority[..=end + 1], after.strip_prefix(':'))
      }
      None => (authority, None),
    };
  }
  match authority.rsplit_once(':') {
    Some((host, port)) if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) => {
      (host, Some(port))
    }
    _ => (authority, None),
  }
}

/// Equal credentials without an early exit on the first differing byte, so a
/// timing oracle cannot walk the token one character at a time. The length
/// check still returns early: a configured token's length stops being secret
/// the moment it exists, and the comparison it guards is over equal-length
/// bytes either way.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
  if left.len() != right.len() {
    return false;
  }
  let mut difference = 0u8;
  for (a, b) in left.iter().zip(right.iter()) {
    difference |= a ^ b;
  }
  difference == 0
}

fn rejection(
  status: StatusCode,
  label: &'static str,
  message: &str,
  method: &Method,
  path: &str,
) -> Response {
  log_denial(label, method, path);
  (status, Json(json!({"error":{"message":message}}))).into_response()
}

/// The first denial of each kind is logged in full: it names a misconfiguration
/// the operator has to fix, and a page hammering a blocked endpoint afterwards
/// only repeats it.
fn log_denial(label: &'static str, method: &Method, path: &str) {
  static LOGGED: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
  let mut logged = match LOGGED.lock() {
    Ok(guard) => guard,
    Err(poisoned) => poisoned.into_inner(),
  };
  if logged.contains(&label) {
    return;
  }
  logged.push(label);
  eprintln!("wish: rejected {method} {path}: {label}");
}

#[cfg(test)]
mod tests {
  use super::*;

  const TOKEN: &str = "t0p-s3cret";

  // No token configured: every write fails closed, whether it comes from a CLI
  // or from the app's own origin in a browser. Reads still answer, because
  // /health and /version style diagnostics must keep working.
  #[test]
  fn writes_fail_closed_without_a_token() {
    for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
      assert_eq!(
        admission(None, &method, None, None, None, None),
        Admission::Unauthenticated,
        "{method} from a CLI must fail closed without a token"
      );
      assert_eq!(
        admission(
          None,
          &method,
          None,
          Some("http://127.0.0.1:9780"),
          Some("127.0.0.1:9780"),
          None
        ),
        Admission::Unauthenticated,
        "{method} from the app's own origin must fail closed without a token"
      );
    }
  }

  #[test]
  fn reads_work_without_a_token_for_diagnostics() {
    for method in [Method::GET, Method::HEAD] {
      assert_eq!(
        admission(None, &method, None, None, None, None),
        Admission::Allow,
        "{method} must stay open without a token"
      );
    }
  }

  // A token is configured: every method, writes included, needs it, and a
  // wrong or absent bearer answers 401 exactly as before.
  #[test]
  fn token_gate_rejects_missing_or_wrong_bearer() {
    assert_eq!(
      admission(Some(TOKEN), &Method::POST, None, None, None, None),
      Admission::Unauthorized
    );
    assert_eq!(
      admission(Some(TOKEN), &Method::POST, Some("wrong"), None, None, None),
      Admission::Unauthorized
    );
    assert_eq!(
      admission(Some(TOKEN), &Method::POST, Some(""), None, None, None),
      Admission::Unauthorized
    );
    assert_eq!(
      admission(Some(TOKEN), &Method::GET, None, None, None, None),
      Admission::Unauthorized
    );
  }

  #[test]
  fn correct_token_allows_writes_from_a_cli() {
    for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
      assert_eq!(
        admission(Some(TOKEN), &method, Some(TOKEN), None, None, None),
        Admission::Allow,
        "{method} with the right token from a CLI must pass"
      );
    }
    assert_eq!(
      admission(Some(TOKEN), &Method::GET, Some(TOKEN), None, None, None),
      Admission::Allow
    );
  }

  // A page on another origin must never write, token or not: the leak of the
  // token into a page must not hand over the agent.
  #[test]
  fn cross_origin_writes_are_rejected_even_with_the_token() {
    let evil = "https://evil.example";
    let host = Some("127.0.0.1:9780");
    assert_eq!(
      admission(Some(TOKEN), &Method::POST, Some(TOKEN), Some(evil), host, Some("cross-site")),
      Admission::CrossOrigin
    );
    // No Sec-Fetch-Site (older browsers, plain curl with an Origin): the
    // Origin/Host mismatch damns it instead.
    assert_eq!(
      admission(Some(TOKEN), &Method::POST, Some(TOKEN), Some(evil), host, None),
      Admission::CrossOrigin
    );
    // And still a cross-origin write when no token is configured at all.
    assert_eq!(
      admission(None, &Method::POST, None, Some(evil), host, None),
      Admission::CrossOrigin
    );
    // A different port on the same host is another origin, not "the app".
    assert_eq!(
      admission(
        Some(TOKEN),
        &Method::POST,
        Some(TOKEN),
        Some("http://127.0.0.1:3000"),
        host,
        None
      ),
      Admission::CrossOrigin
    );
  }

  #[test]
  fn same_origin_writes_pass() {
    assert_eq!(
      admission(
        Some(TOKEN),
        &Method::POST,
        Some(TOKEN),
        Some("http://127.0.0.1:9780"),
        Some("127.0.0.1:9780"),
        None
      ),
      Admission::Allow
    );
    // Case and default ports are normalized by browsers on both sides.
    assert!(origin_is_this_server("http://127.0.0.1", "127.0.0.1:80"));
    assert!(origin_is_this_server("http://127.0.0.1:80", "127.0.0.1"));
    assert!(origin_is_this_server("http://Wish.Local:9780", "wish.local:9780"));
    assert!(origin_is_this_server("https://wish.local", "wish.local:443"));
    assert!(origin_is_this_server("https://wish.local", "wish.local"));
    assert!(origin_is_this_server("http://[::1]:9780", "[::1]:9780"));
    assert!(origin_is_this_server("http://[::1]", "[::1]:80"));
  }

  #[test]
  fn foreign_origins_do_not_match_this_server() {
    assert!(!origin_is_this_server("https://wish.local", "wish.local:80"));
    assert!(!origin_is_this_server("http://wish.local", "wish.local:443"));
    assert!(!origin_is_this_server("https://evil.example", "127.0.0.1:9780"));
    assert!(!origin_is_this_server("ftp://127.0.0.1:9780", "127.0.0.1:9780"));
    assert!(!origin_is_this_server("http://127.0.0.1:9780", "127.0.0.1"));
    assert!(!origin_is_this_server("nonsense", "127.0.0.1:9780"));
    assert!(!origin_is_this_server("http://user@127.0.0.1:9780", "127.0.0.1:9780"));
  }

  #[test]
  fn a_cli_with_no_origin_is_not_cross_origin() {
    assert!(!is_cross_origin(None, Some("127.0.0.1:9780"), None));
    assert!(!is_cross_origin(None, None, None));
    assert!(!is_cross_origin(None, Some("127.0.0.1:9780"), Some("none")));
    assert!(!is_cross_origin(None, Some("127.0.0.1:9780"), Some("same-origin")));
    assert!(!is_cross_origin(None, Some("127.0.0.1:9780"), Some("same-site")));
    // A forged-looking cross-site value with no Origin at all still damns it.
    assert!(is_cross_origin(None, Some("127.0.0.1:9780"), Some("cross-site")));
    // An Origin with no Host cannot be verified and counts as foreign.
    assert!(is_cross_origin(Some("http://127.0.0.1:9780"), None, None));
  }

  #[test]
  fn only_writes_are_gated_by_origin() {
    // GET/HEAD/OPTIONS never trip the origin check: SSE and preflight survive.
    for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
      assert_eq!(
        admission(
          Some(TOKEN),
          &method,
          Some(TOKEN),
          Some("https://evil.example"),
          Some("127.0.0.1:9780"),
          Some("cross-site")
        ),
        Admission::Allow,
        "{method} must not be origin-gated"
      );
    }
    for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
      assert!(mutates(&method));
    }
    for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
      assert!(!mutates(&method));
    }
  }

  #[test]
  fn bearer_header_parsing_keeps_the_documented_scheme() {
    assert_eq!(bearer_of("Bearer abc"), Some("abc"));
    assert_eq!(bearer_of("Bearer "), Some(""));
    assert_eq!(bearer_of("bearer abc"), None);
    assert_eq!(bearer_of("Basic abc"), None);
    assert_eq!(bearer_of("Bearer"), None);
    assert_eq!(bearer_of("abc"), None);
  }

  #[test]
  fn constant_time_eq_matches_only_equal_bytes() {
    assert!(constant_time_eq(b"t0p-s3cret", b"t0p-s3cret"));
    // Differing in the last byte is still a mismatch, not an early true.
    assert!(!constant_time_eq(b"t0p-s3cret", b"t0p-secreu"));
    assert!(!constant_time_eq(b"t0p-s3cret", b"t0p-s3creu"));
    assert!(!constant_time_eq(b"t0p-s3cret", b"t0p"));
    assert!(!constant_time_eq(b"", b"x"));
    assert!(constant_time_eq(b"", b""));
  }

  // The responses a blocked caller sees: 401 with the historical body for a
  // wrong token, a distinct actionable body for the unconfigured case, and 403
  // for the cross-origin write.
  #[tokio::test]
  async fn rejected_requests_get_actionable_responses() {
    let unauthorized =
      rejection(StatusCode::UNAUTHORIZED, "test-unauthorized", "unauthorized", &Method::POST, "/api/x");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let body = axum::body::to_bytes(unauthorized.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&body[..], br#"{"error":{"message":"unauthorized"}}"#);

    let no_token = rejection(
      StatusCode::UNAUTHORIZED,
      "test-no-token",
      "unauthorized: this server has no bearer token; set bearer_token_env in the configuration and restart to enable write operations",
      &Method::POST,
      "/api/x",
    );
    assert_eq!(no_token.status(), StatusCode::UNAUTHORIZED);
    let body = axum::body::to_bytes(no_token.into_body(), usize::MAX).await.unwrap();
    assert!(body.starts_with(br#"{"error":{"message":"unauthorized: this server has no bearer token"#));

    let cross_origin = rejection(
      StatusCode::FORBIDDEN,
      "test-cross-origin",
      "forbidden: cross-origin write requests are rejected; call this API from the server's own origin or a CLI",
      &Method::POST,
      "/api/x",
    );
    assert_eq!(cross_origin.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(cross_origin.into_body(), usize::MAX).await.unwrap();
    assert!(body.starts_with(br#"{"error":{"message":"forbidden: cross-origin write"#));
  }
}
