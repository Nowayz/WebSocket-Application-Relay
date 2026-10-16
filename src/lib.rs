//! WebSocket Application Relay: wire protocol, bounded routing, and service runtime.
mod admin;
mod common;
pub mod config;
mod frame;
pub mod legacy;
pub mod legacy_bridge;
pub mod metrics;
pub mod origin;
pub mod protocol;
pub mod relay;
pub mod server;
mod transport;

pub use config::Config;
pub use relay::Relay;
pub use server::serve;
