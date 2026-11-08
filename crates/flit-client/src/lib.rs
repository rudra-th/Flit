//! The Flit sending side: dial a peer, pin its fingerprint, send files.
//!
//! Pinning is not optional. A peer is identified by the fingerprint advertised
//! in discovery, and a certificate that does not match is a hard failure. There
//! is no "continue anyway" path in this crate.
//!
//! Implementation lands in M1 alongside the TLS server.

#![deny(missing_docs)]

pub use flit_api::{Info, PROTO_VERSION};
