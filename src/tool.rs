//! Built-in tool implementations. Register their specifications in SessionConfig.tools and
//! supply the implementation (or an application dispatcher) to executor::run.
pub mod ask_user;
pub mod bundled;
pub mod history;
pub mod mcp;
pub mod shell;
pub mod snapshot;
pub mod view_image;
pub mod web;
pub mod web_search;
