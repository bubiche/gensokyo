//! The daemon: owns every resident's PTY and emulator and serves clients over a unix socket.

pub mod aware;
mod ingest;
pub mod launch;
mod notify;
pub mod pty;
pub mod registry;
pub mod resident;
pub mod server;
mod shrine;
pub mod store;
