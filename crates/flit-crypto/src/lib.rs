//! Device identity for Flit.
//!
//! A device is an Ed25519 keypair generated on first run. Everything else is
//! derived from the public key:
//!
//! * the **fingerprint** is `SHA-256(public key)` as lowercase hex, and is what
//!   a peer pins during the TLS handshake;
//! * the **device id** is the first 8 bytes of that hash, short enough to put in
//!   a log line and long enough not to collide in a household;
//! * the **safety badge** is four emoji plus four words read from the same hash.
//!   A name on a LAN is trivial to spoof, a badge is not.
//!
//! The badge exists in two forms on purpose. Emoji are fast for a sighted user
//! to compare across two screens; words are what a screen reader can actually
//! pronounce. The pairing screen shows both and reads the words.

mod tls;

pub use tls::{TlsError, TlsIdentity};

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

/// Emoji used for the visual half of the safety badge.
const BADGE_EMOJI: [&str; 32] = [
    "🦊", "🐻", "🐼", "🐨", "🐯", "🦁", "🐮", "🐷", "🐸", "🐵", "🐔", "🐧", "🦉", "🦄", "🐙", "🦋",
    "🌵", "🌲", "🍁", "🌊", "🔥", "⭐", "🌙", "🍎", "🍋", "🍇", "🍄", "🧭", "🎈", "🔔", "🧩", "🎯",
];

/// Words used for the spoken half of the safety badge.
const BADGE_WORDS: [&str; 64] = [
    "ant", "ash", "bay", "bell", "birch", "blue", "bolt", "brook", "cedar", "chalk", "cliff",
    "cloud", "coral", "cove", "creek", "crow", "dawn", "deer", "delta", "dew", "dune", "dusk",
    "eagle", "elm", "ember", "fern", "fig", "fir", "flint", "fog", "fox", "frost", "gale", "glade",
    "glass", "gold", "grove", "hail", "hare", "hawk", "hazel", "heath", "hill", "holly", "ice",
    "iris", "jade", "kite", "lake", "lark", "leaf", "lime", "linen", "lotus", "maple", "marsh",
    "mint", "moss", "oak", "ocean", "olive", "onyx", "opal", "otter",
];

/// Things that can go wrong while creating an identity.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    /// The operating system refused to provide randomness.
    #[error("could not read randomness from the operating system: {0}")]
    Entropy(#[from] getrandom::Error),
}

/// A device identity: an Ed25519 signing key and everything derived from it.
///
/// The secret is held in a [`SigningKey`], which zeroizes on drop.
pub struct Identity {
    signing: SigningKey,
}

impl Identity {
    /// Create a fresh identity from operating-system randomness.
    pub fn generate() -> Result<Self, CryptoError> {
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes)?;
        Ok(Self::from_secret_bytes(bytes))
    }

    /// Rebuild an identity from its stored secret bytes.
    #[must_use]
    pub fn from_secret_bytes(bytes: [u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(&bytes),
        }
    }

    /// The secret bytes, for writing to the platform keychain.
    ///
    /// Callers are responsible for never writing these to a plain file.
    #[must_use]
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.signing.to_bytes()
    }

    /// The Ed25519 public key.
    #[must_use]
    pub fn public_key(&self) -> [u8; 32] {
        self.verifying_key().to_bytes()
    }

    /// The signing key, for the TLS module and for PKCS#8 export.
    pub(crate) fn signing_key(&self) -> &SigningKey {
        &self.signing
    }

    /// The public key as a `VerifyingKey`, for signature checks.
    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    /// `SHA-256(public key)` as 64 lowercase hex characters.
    ///
    /// This is the value pinned during the TLS handshake and the only identity
    /// claim a peer should trust.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        fingerprint_of(&self.public_key())
    }

    /// The first 8 bytes of the fingerprint: a short, stable device id.
    #[must_use]
    pub fn device_id(&self) -> String {
        self.fingerprint()[..16].to_string()
    }

    /// Sign a message with this identity.
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.signing.sign(message).to_bytes()
    }

    /// The four-emoji half of the safety badge.
    #[must_use]
    pub fn badge_emoji(&self) -> String {
        let hash = self.fingerprint_bytes();
        let mut out = String::new();
        for byte in &hash[0..4] {
            out.push_str(BADGE_EMOJI[usize::from(*byte) % BADGE_EMOJI.len()]);
        }
        out
    }

    /// The four-word half of the safety badge, for screen readers and for
    /// reading out loud over a phone call.
    #[must_use]
    pub fn badge_words(&self) -> String {
        let hash = self.fingerprint_bytes();
        let mut parts = Vec::with_capacity(4);
        for byte in &hash[4..8] {
            parts.push(BADGE_WORDS[usize::from(*byte) % BADGE_WORDS.len()]);
        }
        parts.join("-")
    }

    /// Raw fingerprint bytes.
    fn fingerprint_bytes(&self) -> [u8; 32] {
        Sha256::digest(self.public_key()).into()
    }
}

impl std::fmt::Debug for Identity {
    /// Never print the secret, even by accident.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("device_id", &self.device_id())
            .finish_non_exhaustive()
    }
}

/// `SHA-256(public key)` as lowercase hex.
#[must_use]
pub fn fingerprint_of(public_key: &[u8; 32]) -> String {
    hex::encode(Sha256::digest(public_key))
}

/// Verify a detached signature against a public key.
///
/// Uses strict verification, which rejects small-order and non-canonical keys.
#[must_use]
pub fn verify(public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    let sig = Signature::from_bytes(signature);
    key.verify_strict(message, &sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed() -> Identity {
        Identity::from_secret_bytes([7u8; 32])
    }

    #[test]
    fn identity_is_deterministic_from_secret_bytes() {
        assert_eq!(fixed().fingerprint(), fixed().fingerprint());
        assert_eq!(fixed().device_id(), fixed().device_id());
    }

    #[test]
    fn device_id_is_a_prefix_of_the_fingerprint() {
        let id = fixed();
        assert_eq!(id.fingerprint().len(), 64);
        assert_eq!(id.device_id(), id.fingerprint()[..16]);
    }

    #[test]
    fn distinct_keys_have_distinct_fingerprints() {
        let a = Identity::from_secret_bytes([1u8; 32]);
        let b = Identity::from_secret_bytes([2u8; 32]);
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn signatures_verify_and_tampering_is_rejected() {
        let id = fixed();
        let message = b"flit handshake";
        let sig = id.sign(message);
        assert!(verify(&id.public_key(), message, &sig));
        assert!(!verify(&id.public_key(), b"flit handshak3", &sig));

        let mut flipped = sig;
        flipped[0] ^= 1;
        assert!(!verify(&id.public_key(), message, &flipped));
    }

    #[test]
    fn another_identity_cannot_forge_a_signature() {
        let a = fixed();
        let b = Identity::from_secret_bytes([9u8; 32]);
        let sig = b.sign(b"message");
        assert!(!verify(&a.public_key(), b"message", &sig));
    }

    #[test]
    fn round_trips_through_secret_bytes() {
        let original = Identity::from_secret_bytes([3u8; 32]);
        let restored = Identity::from_secret_bytes(original.secret_bytes());
        assert_eq!(original.fingerprint(), restored.fingerprint());
        assert_eq!(original.public_key(), restored.public_key());
    }

    #[test]
    fn badge_has_four_emoji_and_four_words() {
        let id = fixed();
        assert_eq!(id.badge_emoji().chars().count(), 4);
        assert_eq!(id.badge_words().split('-').count(), 4);
        assert_eq!(id.badge_emoji(), fixed().badge_emoji());
        assert_eq!(id.badge_words(), fixed().badge_words());
    }

    #[test]
    fn debug_never_leaks_the_secret() {
        let text = format!("{:?}", fixed());
        assert!(text.contains("device_id"));
        assert!(!text.contains("secret"));
    }
}
