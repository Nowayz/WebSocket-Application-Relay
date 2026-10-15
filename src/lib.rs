//! WebSocket Application Relay: wire protocol, bounded routing, and service runtime.
mod common;
pub mod config;
pub mod legacy;
pub mod legacy_bridge;
pub mod metrics;
pub mod protocol;
pub mod relay;
pub mod server;

pub use config::Config;
pub use relay::Relay;
pub use server::serve;
