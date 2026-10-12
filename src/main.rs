//! Wish's protocol, session engine and HTTP application in one executable.
// A double-clicked application shows no console window; debug builds keep one.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod client;
mod executor;
mod mcp;
mod migration;
mod protocol;
mod server;
mod session;
mod storage;
mod tool;
mod transport;
mod utils;

use protocol::error::Error;

fn main() -> Result<(), Box<dyn std::error::Error>> {
  server::run()
}
