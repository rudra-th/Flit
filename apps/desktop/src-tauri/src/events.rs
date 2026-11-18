//! Engine events, in the shape the window listens for.
//!
//! One channel (`flit://event`), one tagged union. The window switches on
//! `kind`, so a new event is a new variant here and a new `case` there — never
//! a polymorphic payload that every listener has to sniff.

use serde::Serialize;

use crate::dto::{Item, Offered, Peer};

/// The event channel. A constant so the shell and the UI cannot disagree.
pub const CHANNEL: &str = "flit://event";

/// Something that happened in the engine, ready to be serialised.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum EngineEvent {
    /// A device appeared, or its announce changed.
    PeerSeen {
        /// The device, as the list shows it.
        peer: Peer,
    },
    /// A device stopped answering.
    PeerLost {
        /// Identifier of the device that went away.
        device_id: String,
    },
    /// Discovery could not start. Sending and receiving are unaffected.
    DiscoveryUnavailable {
        /// Why, in words a support issue can quote.
        reason: String,
    },
    /// Another device is asking to send something, and is waiting for an
    /// answer. Only sent when the policy did not answer on its own.
    IncomingOffer {
        /// Which question, for the answer.
        id: u64,
        /// The name it claims. Never proof of anything.
        sender: String,
        /// The key it proved, or empty when it proved none.
        fingerprint: String,
        /// What it wants to send, in order.
        items: Vec<Offered>,
        /// Total payload bytes.
        bytes: u64,
    },
    /// An incoming offer reached a final answer, without this window giving it.
    OfferClosed {
        /// Which question.
        id: u64,
        /// `accepted`, `declined` or `timedOut`, in that spelling.
        outcome: String,
    },
    /// A send was handed to the network.
    TransferStarted {
        /// Which send, as a number the window can key rows by.
        id: u64,
        /// The first file's name, or the folder the files came out of.
        label: String,
        /// How many files it covers.
        files: usize,
        /// Total bytes declared by the local files.
        bytes: u64,
        /// Receiver address, as `ip:port`.
        peer: String,
    },
    /// A send moved forward.
    TransferProgress {
        /// Which send.
        id: u64,
        /// Bytes handed to the network.
        sent: u64,
        /// Bytes the send covers in total.
        total: u64,
        /// Files with a final result.
        files_done: usize,
        /// Files in the send.
        files_total: usize,
    },
    /// Every item of a send has a result.
    TransferFinished {
        /// Which send.
        id: u64,
        /// One entry per file, in the order they were offered.
        items: Vec<Item>,
    },
    /// The session itself failed, before any per-item result existed.
    TransferFailed {
        /// Which send.
        id: u64,
        /// Why, in the sender's words.
        reason: String,
    },
    /// A send was stopped by the user.
    TransferCancelled {
        /// Which send.
        id: u64,
    },
}
