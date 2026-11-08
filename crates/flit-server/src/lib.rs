//! The Flit receiving side: a TLS server that accepts uploads.
//!
//! The server owns four things: identity, sessions, the destination root, and
//! verification. It never trusts a declared size, never opens a path before
//! sanitizing it, and never renames a `.part` file into place until the hash
//! matches.
//!
//! Implementation lands in M1. This crate exists from M0 so the boundary
//! between "receiving" and "sending" is fixed before either is written.

#![deny(missing_docs)]

pub use flit_api::{Info, PROTO_VERSION};
