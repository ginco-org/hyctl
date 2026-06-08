pub mod auth;
pub mod config;
pub mod download;
pub mod launch;
pub mod session;
pub mod wharf;

pub const BIN_NAME: &str = env!("CARGO_PKG_NAME");
