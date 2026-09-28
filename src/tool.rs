//! Built-in tool implementations. Register their specifications in SessionConfig.tools and
//! supply the implementation (or an application dispatcher) to executor::run.
pub mod mcp;
pub mod search_history;
pub mod shell;
pub mod snapshot;
pub mod view_image;
pub mod web;
