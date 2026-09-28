//! Wish's protocol, session engine and HTTP application in one executable.
// A double-clicked application shows no console window; debug builds keep one.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

pub mod executor;
pub mod protocol;
mod server;
pub mod session;
pub mod storage;
pub mod tool;
pub mod transport;
pub mod utils;

pub use protocol::error::Error;
pub use utils::retry::RetryPolicy;

fn main() -> Result<(), Box<dyn std::error::Error>> {
  server::run()
}
