//! The daemon: owns every resident's PTY and emulator and serves clients over a unix socket.

pub mod aware;
pub mod cards;
mod headless;
mod ingest;
mod keep;
pub mod launch;
mod log;
mod notify;
pub mod pty;
pub mod registry;
mod resident;
mod rituals;
pub mod server;
mod shrine;
pub mod store;
mod stream;
