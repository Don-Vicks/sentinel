//! Vortex Sentinel: real-time incident detection for Solana programs.
//!
//! Sentinel consumes decoded transactions from Vortex through the
//! [`source::VortexSource`] boundary and turns them into metrics, incidents,
//! investigations and alerts.

pub mod alerts;
pub mod analyze;
pub mod api;
pub mod detect;
pub mod engine;
pub mod live;
pub mod metrics;
pub mod simulate;
pub mod model;
pub mod pricing;
pub mod source;
pub mod store;
pub mod trace;
