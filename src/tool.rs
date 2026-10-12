//! Built-in tool implementations. Register their specifications in SessionConfig.tools and
//! supply the implementation (or an application dispatcher) to executor::run.
pub mod ask_user;
pub mod bundled;
pub mod history;
// wish-enhanced note: a model-visible, namespaced `mcp_*` tool layer used to live
// here. Upstream replaced it with `server::mcp::McpHub` (per-session and shared
// instances, stdio and HTTP transports, idle reaping) reached by a session's shell
// through the `wish mcp` bridge, so the old module was removed. It is still in
// history (`git show bc4d730:src/tool/mcp.rs`) if model-visible tools come back.
pub mod shell;
pub mod snapshot;
pub mod view_image;
pub mod web_search;
