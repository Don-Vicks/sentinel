//! Vortex Sentinel: real-time incident detection for Solana programs.
//!
//! Sentinel consumes decoded transactions from Vortex through the
//! [`source::VortexSource`] boundary and turns them into metrics, incidents,
//! investigations and alerts.

pub mod alerts;
pub mod analyze;
pub mod auth;
pub mod beam;
pub mod catalog;
pub mod api;
pub mod detect;
pub mod engine;
pub mod idl;
pub mod limits;
pub mod live;
pub mod metrics;
pub mod mirage_setup;
pub mod simulate;
pub mod model;
pub mod pricing;
pub mod redact;
pub mod resolve;
pub mod source;
pub mod store;
pub mod trace;
pub mod writer;
