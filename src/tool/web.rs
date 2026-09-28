//! Web access tools: search, fetch and cached paging of remote content.
//! One provider serves search; fetch speaks plain HTTP with SSRF restrictions.
//! Fetched pages and search results are cached in memory for one hour.

use crate::executor::tool::{ToolCall, ToolExecutor, ToolOutcome};
use crate::protocol::Tool;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;

/// Which search backend answers `web_search`.
#[derive(Clone, Copy, Debug)]
pub enum SearchProvider {
  /// The HTML endpoint; needs no API key.
  DuckDuckGo,
  Tavily,
  Brave,
}

/// Web tool configuration, resolved from the application config.
#[derive(Clone)]
pub struct WebConfig {
  pub provider: SearchProvider,
  pub fetch_timeout: Duration,
  /// Environment variable holding the provider's API key, when one is needed.
  pub api_key_env: Option<String>,
}

const USER_AGENT: &str =
  "Mozilla/5.0 (compatible; wish-agent/1.0; +https://github.com/WindustH/wish-core)";
struct Cached {
  text: String,
  fetched_at: Instant,
}
const CACHE_TTL: Duration = Duration::from_secs(60 * 60);
const CACHE_MAX: usize = 128;
const FETCH_LIMIT: usize = 4 * 1024 * 1024;

/// The web_search / fetch_content / get_search_content tool set.
pub struct WebTool {
  config: WebConfig,
  cache: Arc<Mutex<HashMap<u64, Cached>>>,
  next_id: AsyncMutex<u64>,
}

impl WebTool {
  pub fn new(config: WebConfig) -> Self {
    Self { config, cache: Arc::new(Mutex::new(HashMap::new())), next_id: AsyncMutex::new(1) }
  }

  pub fn get_specifications(&self) -> Vec<Tool> {
    vec![
      Tool {
        name: "web_search".into(),
        description: "Search the web. Returns {id, results:[{title,url,snippet}]}; pass the id           to get_search_content to read a result in full."
          .into(),
        input_schema: json!({
          "type": "object",
          "properties": {
            "query": {"type": "string", "description": "The search query"},
            "num_results": {"type": "integer", "minimum": 1, "maximum": 20,
              "description": "How many results to return (default 8)"}
          },
          "required": ["query"]
        }),
      },
      Tool {
        name: "fetch_content".into(),
        description: "Fetch a web page and return its content as text. HTML is stripped to           readable text; binary content is refused. The page is cached for one hour."
          .into(),
        input_schema: json!({
          "type": "object",
          "properties": {
            "url": {"type": "string", "description": "Absolute http(s) URL to fetch"},
            "mode": {"type": "string", "enum": ["text", "raw"],
              "description": "text strips markup (default), raw returns the body unchanged"}
          },
          "required": ["url"]
        }),
      },
      Tool {
        name: "get_search_content".into(),
        description: "Read part of a cached search result or fetched page by its response id,           optionally returning the region around a found text."
          .into(),
        input_schema: json!({
          "type": "object",
          "properties": {
            "id": {"type": "integer", "description": "The response id from a prior call"},
            "offset": {"type": "integer", "minimum": 0,
              "description": "Byte offset to start reading at (default 0)"},
            "max_bytes": {"type": "integer", "minimum": 1,
              "description": "Maximum bytes to return (default 16384)"},
            "find": {"type": "string", "description": "Return the region around this text"}
          },
          "required": ["id"]
        }),
      },
    ]
  }

  async fn store(&self, text: String) -> u64 {
    let mut id = self.next_id.lock().await;
    let current = *id;
    *id += 1;
    drop(id);
    let mut cache = self.cache.lock().unwrap();
    if cache.len() >= CACHE_MAX && let Some(oldest) =
      cache.iter().min_by_key(|(_, cached)| cached.fetched_at).map(|(key, _)| *key)
    {
      cache.remove(&oldest);
    }
    cache.insert(current, Cached { text, fetched_at: Instant::now() });
    current
  }

  fn cached_text(&self, id: u64) -> Option<String> {
    let cache = self.cache.lock().unwrap();
    let cached = cache.get(&id)?;
    (cached.fetched_at.elapsed() < CACHE_TTL).then(|| cached.text.clone())
  }

  fn http(&self) -> Result<reqwest::Client, ToolOutcome> {
    reqwest::Client::builder()
      .timeout(self.config.fetch_timeout)
      .user_agent(USER_AGENT)
      .build()
      .map_err(|error| ToolOutcome::Failed(format!("http client error: {error}")))
  }

  async fn search(&self, arguments: &Value) -> ToolOutcome {
    let Some(query) = arguments.get("query").and_then(Value::as_str) else {
      return ToolOutcome::Failed("query is required".into());
    };
    let num = arguments.get("num_results").and_then(Value::as_u64).unwrap_or(8).min(20) as usize;
    match self.config.provider {
      SearchProvider::DuckDuckGo => self.search_ddg(query, num).await,
      SearchProvider::Tavily => self.search_tavily(query, num).await,
      SearchProvider::Brave => self.search_brave(query, num).await,
    }
  }

  async fn search_ddg(&self, query: &str, num: usize) -> ToolOutcome {
    let client = match self.http() { Ok(client) => client, Err(outcome) => return outcome };
    let body = match client.post("https://html.duckduckgo.com/html/").form(&[("q", query)]).send().await {
      Ok(response) if response.status().is_success() => response.text().await.unwrap_or_default(),
      Ok(response) => {
        return ToolOutcome::Failed(format!("duckduckgo returned HTTP {}", response.status()))
      }
      Err(error) => return ToolOutcome::Failed(format!("duckduckgo request failed: {error}")),
    };
    let results = parse_ddg(&body, num);
    if results.is_empty() {
      return ToolOutcome::Failed("duckduckgo returned no results".into());
    }
    let rendered = json!({"query": query, "results": results});
    let id = self.store(rendered.to_string()).await;
    ToolOutcome::Success(json!({"id": id, "results": results}))
  }

  async fn search_tavily(&self, query: &str, num: usize) -> ToolOutcome {
    let Some(key) = self.config.api_key_env.as_deref().and_then(|name| std::env::var(name).ok()).filter(|k| !k.is_empty()) else {
      return ToolOutcome::Failed("tavily API key is not configured".into());
    };
    let client = match self.http() { Ok(client) => client, Err(outcome) => return outcome };
    let response = client
      .post("https://api.tavily.com/search")
      .bearer_auth(&key)
      .json(&json!({"query": query, "search_depth": "basic", "max_results": num}))
      .send()
      .await;
    let body = match response {
      Ok(response) if response.status().is_success() => response.text().await.unwrap_or_default(),
      Ok(response) => return ToolOutcome::Failed(format!("tavily returned HTTP {}", response.status())),
      Err(error) => return ToolOutcome::Failed(format!("tavily request failed: {error}")),
    };
    let parsed: Value = match serde_json::from_str(&body) {
      Ok(value) => value,
      Err(error) => return ToolOutcome::Failed(format!("tavily body parse failed: {error}")),
    };
    let results: Vec<Value> = parsed
      .get("results")
      .and_then(Value::as_array)
      .map(|items| {
        items
          .iter()
          .take(num)
          .map(|item| {
            json!({
              "title": item.get("title").cloned().unwrap_or(Value::Null),
              "url": item.get("url").cloned().unwrap_or(Value::Null),
              "snippet": item.get("content").cloned().unwrap_or(Value::Null)
            })
          })
          .collect()
      })
      .unwrap_or_default();
    let rendered = json!({"query": query, "results": results});
    let id = self.store(rendered.to_string()).await;
    ToolOutcome::Success(json!({"id": id, "results": results}))
  }

  async fn search_brave(&self, query: &str, num: usize) -> ToolOutcome {
    let Some(key) = self.config.api_key_env.as_deref().and_then(|name| std::env::var(name).ok()).filter(|k| !k.is_empty()) else {
      return ToolOutcome::Failed("brave API key is not configured".into());
    };
    let client = match self.http() { Ok(client) => client, Err(outcome) => return outcome };
    let response = client
      .get("https://api.search.brave.com/res/v1/web/search")
      .header("X-Subscription-Token", &key)
      .query(&[("q", query), ("count", &num.to_string())])
      .send()
      .await;
    let body = match response {
      Ok(response) if response.status().is_success() => response.text().await.unwrap_or_default(),
      Ok(response) => return ToolOutcome::Failed(format!("brave returned HTTP {}", response.status())),
      Err(error) => return ToolOutcome::Failed(format!("brave request failed: {error}")),
    };
    let parsed: Value = match serde_json::from_str(&body) {
      Ok(value) => value,
      Err(error) => return ToolOutcome::Failed(format!("brave body parse failed: {error}")),
    };
    let results: Vec<Value> = parsed
      .pointer("/web/results")
      .and_then(Value::as_array)
      .map(|items| {
        items
          .iter()
          .take(num)
          .map(|item| {
            json!({
              "title": item.get("title").cloned().unwrap_or(Value::Null),
              "url": item.get("url").cloned().unwrap_or(Value::Null),
              "snippet": item.get("description").cloned().unwrap_or(Value::Null)
            })
          })
          .collect()
      })
      .unwrap_or_default();
    let rendered = json!({"query": query, "results": results});
    let id = self.store(rendered.to_string()).await;
    ToolOutcome::Success(json!({"id": id, "results": results}))
  }

  async fn fetch(&self, arguments: &Value) -> ToolOutcome {
    let Some(url) = arguments.get("url").and_then(Value::as_str) else {
      return ToolOutcome::Failed("url is required".into());
    };
    if let Some(problem) = ssrf_violation(url) {
      return ToolOutcome::Failed(format!("refused: {problem}"));
    }
    let raw = arguments.get("mode").and_then(Value::as_str) == Some("raw");
    let client = match self.http() { Ok(client) => client, Err(outcome) => return outcome };
    let response = client.get(url).send().await;
    let mut response = match response {
      Ok(response) => response,
      Err(error) => return ToolOutcome::Failed(format!("fetch failed: {error}")),
    };
    let content_type = response
      .headers()
      .get(reqwest::header::CONTENT_TYPE)
      .and_then(|value| value.to_str().ok())
      .unwrap_or("")
      .to_owned();
    if content_type.starts_with("application/pdf")
      || content_type.starts_with("image/")
      || content_type.starts_with("video/")
      || content_type.starts_with("audio/")
      || content_type.starts_with("application/octet-stream")
    {
      return ToolOutcome::Failed(format!(
        "binary content ({content_type}); download it with shell commands instead"
      ));
    }
    // Stream-read with a hard cap so a huge body cannot exhaust memory.
    let mut body = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
      body.extend_from_slice(&chunk);
      if body.len() > FETCH_LIMIT {
        return ToolOutcome::Failed(format!("page exceeds {FETCH_LIMIT} bytes"));
      }
    }
    let text = String::from_utf8_lossy(&body).into_owned();
    let text = if raw { text } else { strip_html(&text) };
    let id = self.store(text.clone()).await;
    ToolOutcome::Success(json!({"id": id, "url": url, "bytes": text.len(), "text": text}))
  }

  fn page(&self, arguments: &Value) -> ToolOutcome {
    let Some(id) = arguments.get("id").and_then(Value::as_u64) else {
      return ToolOutcome::Failed("id is required".into());
    };
    let Some(text) = self.cached_text(id) else {
      return ToolOutcome::Failed(format!("no cached content for id {id} (expired after 1h?)"));
    };
    let offset = arguments.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    if offset > text.len() {
      return ToolOutcome::Failed(format!("offset {offset} is past the end ({})", text.len()));
    }
    let max = arguments.get("max_bytes").and_then(Value::as_u64).unwrap_or(16 * 1024) as usize;
    let remainder = &text[offset.min(text.len())..];
    let (chunk, next_offset) = if let Some(find) = arguments.get("find").and_then(Value::as_str) {
      match remainder.find(find) {
        Some(position) => {
          let start = position.saturating_sub(512);
          let end = (position + find.len() + 4096).min(remainder.len());
          (remainder[start..end].to_owned(), Some(offset + end))
        }
        None => (remainder[..max.min(remainder.len())].to_owned(), None),
      }
    } else {
      let end = max.min(remainder.len());
      let next = (offset + end < text.len()).then(|| offset + end);
      (remainder[..end].to_owned(), next)
    };
    ToolOutcome::Success(json!({
      "id": id, "offset": offset, "bytes": chunk.len(),
      "next_offset": next_offset, "total": text.len(), "text": chunk
    }))
  }
}

impl ToolExecutor for WebTool {
  async fn execute(
    &self,
    call: &ToolCall,
    _control: &crate::executor::ExecutionControl,
  ) -> ToolOutcome {
    match call.name.as_str() {
      "web_search" => self.search(&call.arguments).await,
      "fetch_content" => self.fetch(&call.arguments).await,
      "get_search_content" => self.page(&call.arguments),
      _ => ToolOutcome::Failed(format!("unknown web tool: {}", call.name)),
    }
  }
}

/// Refuses URLs that resolve to loopback or private ranges. DNS is resolved by the
/// HTTP client later; this catches the literal-host cases the config surface exposes.
fn ssrf_violation(url: &str) -> Option<&'static str> {
  let parsed = reqwest::Url::parse(url).ok()?;
  if !matches!(parsed.scheme(), "http" | "https") {
    return Some("only http and https URLs are allowed");
  }
  match parsed.host_str()? {
    "localhost" => Some("localhost is not reachable from fetch_content"),
    host => {
      let literal = host.trim_start_matches('[').trim_end_matches(']');
      if let Ok(ip) = literal.parse::<IpAddr>() {
        classify_ip(&ip)
      } else {
        None
      }
    }
  }
}

fn classify_ip(ip: &IpAddr) -> Option<&'static str> {
  match ip {
    IpAddr::V4(v4) => {
      let octets = v4.octets();
      if v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_broadcast()
        || octets[0] == 0
        || (octets[0] == 100 && (octets[1] & 0b1100_0000) == 64)
        || octets[0] == 169 && octets[1] == 254
      {
        Some("private or loopback addresses are not reachable from fetch_content")
      } else {
        None
      }
    }
    IpAddr::V6(v6) => {
      let segments = v6.segments();
      if v6.is_loopback()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
      {
        Some("private or loopback addresses are not reachable from fetch_content")
      } else {
        None
      }
    }
  }
}

/// Extracts result links and snippets from the DuckDuckGo HTML endpoint.
fn parse_ddg(body: &str, num: usize) -> Vec<Value> {
  let mut results = Vec::new();
  for part in body.split("class=\"result__a\"") {
    if results.len() >= num {
      break;
    }
    let Some(href_start) = part.find("href=\"") else { continue };
    let after = &part[href_start + 6..];
    let Some(href_end) = after.find('"') else { continue };
    let url = decode_entities(&after[..href_end]);
    if !url.starts_with("http") {
      continue;
    }
    let Some(title_start) = part.find('>') else { continue };
    let after_title = &part[title_start + 1..];
    let Some(title_end) = after_title.find("</a>") else { continue };
    let title = decode_entities(&after_title[..title_end]);
    let snippet = part
      .find("class=\"result__snippet\"")
      .and_then(|start| {
        let segment = &part[start..];
        let gt = segment.find('>')?;
        let end = segment[gt + 1..].find("</a>")?;
        Some(decode_entities(&segment[gt + 1..gt + 1 + end]))
      })
      .unwrap_or_default();
    results.push(json!({"title": title, "url": url, "snippet": snippet}));
  }
  results
}

/// Unescapes the handful of HTML entities the parser depends on.
fn decode_entities(input: &str) -> String {
  input
    .replace("&amp;", "&")
    .replace("&lt;", "<")
    .replace("&gt;", ">")
    .replace("&quot;", "\"")
    .replace("&#x27;", "'")
    .replace("&#39;", "'")
}

/// Strips markup into readable text: script, style and tag soup out, block tags to newlines.
fn strip_html(html: &str) -> String {
  let mut output = String::with_capacity(html.len());
  let mut rest = html.to_owned();
  // Remove script and style elements entirely.
  for remove in ["script", "style", "noscript", "head"] {
    let open = format!("<{remove}");
    let close = format!("</{remove}>");
    while let Some(start) = rest.to_ascii_lowercase().find(&open) {
      let Some(end_rel) = rest[start..].to_ascii_lowercase().find(&close) else { break };
      rest = format!("{}{}", &rest[..start], &rest[start + end_rel + close.len()..]);
    }
  }
  let mut inside_tag = false;
  for character in rest.chars() {
    match character {
      '<' => {
        inside_tag = true;
        output.push(' ');
      }
      '>' => inside_tag = false,
      _ if !inside_tag => output.push(character),
      _ => {}
    }
  }
  let collapsed = output
    .split('\n')
    .map(str::trim)
    .filter(|line| !line.is_empty())
    .collect::<Vec<_>>()
    .join("\n");
  collapsed
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::net::IpAddr;

  #[test]
  fn decodes_common_entities() {
    assert_eq!(decode_entities("a&amp;b&quot;c&#x27;d"), "a&b\"c'd");
  }

  #[test]
  fn parses_ddg_result_blocks() {
    let fixture = r#"<div class="result"><a class="result__a" href="https://example.com/a">First &amp; Foremost</a><a class="result__snippet">alpha snippet</a></div>
      <a class="result__a" href="/redirect?u=https://example.org/b">Second</a>"#;
    let results = parse_ddg(fixture, 10);
    assert_eq!(results.len(), 1, "relative hrefs are skipped");
    assert_eq!(results[0]["url"], "https://example.com/a");
    assert_eq!(results[0]["title"], "First & Foremost");
  }

  #[test]
  fn strips_scripts_styles_and_tags() {
    let html = "<style>.x{}</style><script>evil()</script><h1>Title</h1><p>Body text.</p>";
    let text = strip_html(html);
    assert!(!text.contains("evil"));
    assert!(!text.contains(".x"));
    assert!(text.contains("Title"));
    assert!(text.contains("Body text."));
  }

  #[test]
  fn classifies_private_targets_as_unreachable() {
    for literal in ["127.0.0.1", "10.0.0.5", "192.168.1.1", "172.16.0.9", "169.254.169.254", "100.64.0.1", "::1", "fd00::5", "fe80::1"] {
      let ip: IpAddr = literal.parse().unwrap();
      assert!(classify_ip(&ip).is_some(), "{literal} must be blocked");
    }
    let public: IpAddr = "1.1.1.1".parse().unwrap();
    assert!(classify_ip(&public).is_none());
  }
}