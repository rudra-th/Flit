//! The Flit engine facade.
//!
//! Every UI — Tauri desktop, the Android shell, the CLI — talks to the engine
//! through this crate and nothing below it. The boundary exists so the UI can
//! be replaced without touching the engine, and so the engine can be exposed
//! over UniFFI later without redesign.
//!
//! Two rules hold here and are enforced by review:
//!
//! * the engine never imports a UI crate;
//! * the UI never implements protocol logic.
//!
//! M0 defines the boundary only. `start`, `send`, `receive`, the event stream
//! and the doctor land alongside their transport in M1–M2.

#![deny(missing_docs)]

pub use flit_api::{Info, PROTO_VERSION};
pub use flit_crypto::{CryptoError, Identity};

/// Build metadata reported by the engine and by `flit doctor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildInfo {
    /// Crate version, from `CARGO_PKG_VERSION`.
    pub version: &'static str,
    /// Wire protocol version this build speaks.
    pub proto: u16,
}

impl BuildInfo {
    /// The build this binary was compiled from.
    #[must_use]
    pub const fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            proto: PROTO_VERSION,
        }
    }
}
