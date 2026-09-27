//! The daemon: owns every resident's PTY and emulator and serves clients over a unix socket.

pub mod aware;
pub mod launch;
pub mod pty;
pub mod registry;
pub mod resident;
pub mod server;
pub mod store;
