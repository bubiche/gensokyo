//! The TUI client: reads the host terminal, draws the shrine and the resident on screen, and
//! talks to the daemon over the same socket as the CLI.

pub mod app;
pub mod framer;
pub mod keys;
pub mod modal;
pub mod render;
