//! Discovery for Flit.
//!
//! Two paths, in this order:
//!
//! 1. a JSON announce sent to the multicast group every two seconds;
//! 2. a subnet scan of `GET /api/v1/info` over the local /24 when multicast is
//!    blocked, which is the common case on guest and corporate networks.
//!
//! QR and manual entry are the last resort and also the way to cross subnets.
//!
//! Implementation lands in M2. This crate exists from M0 so the workspace
//! layout is fixed and the module boundary is honest.

#![deny(missing_docs)]

pub use flit_api::{Announce, DISCOVERY_PORT, MULTICAST_GROUP};
