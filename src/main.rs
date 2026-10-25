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

// Every module reaches for the protocol error as `crate::Error`; the root
// re-exports it for them.
pub(crate) use protocol::error::Error;

fn main() {
  let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build();
  let rt = match rt {
    Ok(rt) => rt,
    Err(error) => {
      server::browser::report_failure(&format!("could not start the runtime: {error}"));
      std::process::exit(1);
    }
  };
  if let Err(error) = rt.block_on(server::run()) {
    // A double-clicked release build has no console: without this the failure
    // is a window that never appears.
    server::browser::report_failure(&error.to_string());
    std::process::exit(1);
  }
}
