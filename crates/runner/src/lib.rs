//! Telehand runner: connects to a server and executes file tools locally.

mod client;
pub mod config;

pub use client::{RunExit, run};
pub use config::RunnerConfig;
