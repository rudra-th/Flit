//! Consent: what the receiver is asked before any bytes move.
//!
//! The receiver owns the question and the answer, but not the policy. A shell
//! injects an [`OfferGate`] that decides whether this sender is already trusted,
//! whether the person said yes, or whether nobody answered. This crate only
//! knows how to carry the offer and how to honour the answer.
//!
//! The sender's identity comes from the client certificate on the TLS
//! connection, never from a header or a JSON field: a claimed name is a label,
//! and consent that binds to a label binds to nothing.

use std::{future::Future, net::IpAddr, pin::Pin, sync::Arc};

use flit_api::ManifestItem;

/// A transfer offered by a sender, before a session exists.
#[derive(Debug, Clone)]
pub struct Offer {
    /// Source address of the connection.
    pub peer: IpAddr,
    /// SHA-256 of the key in the sender's client certificate, when it
    /// presented one.
    pub fingerprint: Option<String>,
    /// Sender's own device id, as claimed. Cosmetic.
    pub device_id: Option<String>,
    /// Sender's display name, as claimed. Cosmetic.
    pub alias: Option<String>,
    /// Everything the sender wants to send.
    pub items: Vec<ManifestItem>,
    /// Sum of the items' payload bytes, for a free-space check.
    pub bytes: u64,
}

impl Offer {
    /// Every item id, in the order the sender offered them.
    #[must_use]
    pub fn all_ids(&self) -> Vec<String> {
        self.items.iter().map(|item| item.id.clone()).collect()
    }
}

/// What the receiver decided about an offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfferDecision {
    /// Open a session for exactly these item ids.
    Accept(Vec<String>),
    /// Refuse the whole transfer.
    Decline,
    /// Nobody answered.
    ///
    /// Kept apart from [`Self::Decline`] because the two read differently to a
    /// person: a decline is an answer, silence is not, and a sender that got no
    /// answer may offer again later without overriding a decision.
    NoAnswer,
}

/// The async policy hook a shell installs on the receiver.
///
/// A closure rather than a trait so the shell can capture its own event sender,
/// pending-answer map and trust list without a second layer of types.
pub type OfferGate =
    Arc<dyn Fn(Offer) -> Pin<Box<dyn Future<Output = OfferDecision> + Send>> + Send + Sync>;

/// Everything the receiver knows about the sender when an offer arrives.
#[derive(Debug, Clone)]
pub struct OfferContext {
    /// Source address of the connection.
    pub peer: IpAddr,
    /// Fingerprint proven by the client certificate, if one was presented.
    pub fingerprint: Option<String>,
    /// Claimed device id.
    pub device_id: Option<String>,
    /// Claimed display name.
    pub alias: Option<String>,
}
