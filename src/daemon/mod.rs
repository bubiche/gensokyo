//! The daemon: owns every resident's PTY and emulator and serves clients over a unix socket.

mod awake;
pub mod aware;
mod branch;
pub mod cards;
mod crash;
mod headless;
mod ingest;
mod keep;
pub mod launch;
mod lead;
mod log;
mod notify;
mod probe;
pub mod pty;
pub mod registry;
mod renew;
mod resident;
mod rituals;
pub mod server;
mod shrine;
pub mod store;
mod stream;
mod tags;
mod worktree;
