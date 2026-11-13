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
//! The shape is the same for every shell:
//!
//! ```no_run
//! # async fn example(identity: std::sync::Arc<flit_core::Identity>) -> Result<(), flit_core::EngineError> {
//! use flit_core::{Engine, EngineConfig, Event};
//!
//! let engine = Engine::start(identity, EngineConfig::default()).await?;
//! let mut events = engine.subscribe();
//! let paths = vec![std::path::PathBuf::from("photo.jpg")];
//! let collected = flit_core::collect_targets(&paths, None).expect("readable files");
//! // `send_to_peer` for a device from discovery, `send_to` for a QR code.
//! # drop(collected);
//! while let Ok(event) = events.recv().await {
//!     match event {
//!         Event::PeerSeen(peer) => println!("{} at {}", peer.alias, peer.addr),
//!         Event::TransferFinished { id, items } => {
//!             println!("{id}: {} item(s)", items.len());
//!             break;
//!         }
//!         _ => {}
//!     }
//! }
//! engine.stop().await;
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]

mod engine;
mod targets;

pub use engine::{
    DiscoveryState, EVENT_BUFFER, Engine, EngineConfig, EngineError, Event, ItemOutcome, PeerView,
    TransferId,
};
pub use targets::{CollectError, Collected, collect_targets};

pub use flit_api::{Info, PROTO_VERSION};
pub use flit_client::SendTarget;
pub use flit_crypto::{CryptoError, Identity};
pub use flit_discovery::PeerSource;

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
