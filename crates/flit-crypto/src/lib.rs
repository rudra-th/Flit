//! Device identity for Flit.
//!
//! A device is an Ed25519 keypair generated on first run. Everything else is
//! derived from the public key:
//!
//! * the **fingerprint** is `SHA-256(public key)` as lowercase hex, and is what
//!   a peer pins during the TLS handshake;
//! * the **device id** is the first 8 bytes of that hash, short enough to put in
//!   a log line and long enough not to collide in a household;
//! * the **name** is two words (`amber heron`), stored on the device and
//!   editable. It is a label, not an identity: two screens showing the same
//!   name prove nothing, which is why the fingerprint is what a peer pins.
//!
//! The name is generated once, on first run, and then belongs to the person
//! using the device. It lives beside the key so the desktop shell, the CLI and
//! the phone all announce the same one.

mod store;
mod tls;

pub use store::{IdentityFile, NameFile, StoreError};
pub use tls::{TlsError, TlsIdentity, ed25519_public_key};

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

/// First half of a device name: qualities, materials, weather.
#[rustfmt::skip]
const NAME_QUALIFIERS: &[&str] = &[
    "amber", "ancient", "arctic", "ashen", "autumn", "azure", "basalt", "bay", "beech", "bitter",
    "black", "blank", "blue", "bold", "boreal", "braided", "brave", "breezy", "bright", "brisk",
    "bronze", "brown", "burnt", "calm", "candied", "cedar", "chalk", "cherry", "chestnut",
    "chilled", "chrome", "cinder", "cinnamon", "clover", "cobalt", "cold", "copper", "coral",
    "cotton", "crimson", "crisp", "crystal", "damp", "dappled", "dark", "dawn", "deep", "dense",
    "dewy", "distant", "dry", "dusk", "dusty", "earthen", "eastern", "ebony", "elder", "electric",
    "ember", "empty", "evening", "faint", "fair", "fallen", "far", "feathered", "felt", "fern",
    "fiery", "fine", "fir", "first", "flint", "floating", "foggy", "forest", "free", "fresh",
    "frost", "frozen", "full", "gilded", "glass", "glazed", "golden", "grand", "granite",
    "gravel", "gray", "green", "gusty", "hazy", "heather", "heavy", "hidden", "high", "hollow",
    "honey", "humble", "icy", "indigo", "inland", "iron", "ivory", "jade", "keen", "kind", "late",
    "lavender", "lazy", "leaf", "light", "lilac", "lime", "linen", "little", "lively", "long",
    "lost", "loud", "low", "lunar", "lush", "maple", "marble", "marsh", "matte", "meadow",
    "mellow", "mild", "misty", "mixed", "mocha", "modest", "morning", "moss", "muted", "narrow",
    "nettle", "new", "night", "nimble", "noon", "northern", "oak", "ochre", "olive", "onyx",
    "open", "opal", "orange", "orchard", "pale", "paper", "patient", "peach", "pearl", "pebble",
    "pine", "pink", "plain", "plum", "polar", "polished", "pond", "pressed", "prime", "proud",
    "purple", "quiet", "rain", "rapid", "rare", "raw", "red", "reed", "rich", "ripe", "river",
    "roasted", "rocky", "rose", "rough", "round", "royal", "ruby", "rugged", "russet", "rust",
    "rye", "salt", "sand", "sapphire", "satin", "scarlet", "sea", "second", "secret", "shadow",
    "shallow", "sharp", "shy", "silent", "silk", "silver", "simple", "slate", "slow", "small",
    "smooth", "snow", "soft", "solar", "solid", "soot", "sour", "southern", "spare", "spring",
    "spruce", "spun", "star", "steel", "still", "stone", "stormy", "straight", "stray", "strong",
    "straw", "summer", "sun", "sunny", "tall", "tan", "tapered", "tart", "teal", "tender", "thin",
    "thorn", "tidy", "tin", "topaz", "tranquil", "true", "tulip", "tundra", "turquoise",
    "twilight", "umber", "upland", "vale", "velvet", "verdant", "violet", "vivid", "warm", "wax",
    "weathered", "western", "wheat", "white", "wide", "wild", "willow", "windy", "winter", "wire",
    "witty", "woven", "yellow", "young", "zinc",
];

/// Second half of a device name: things that flit.
#[rustfmt::skip]
const NAME_CREATURES: &[&str] = &[
    "adder", "albatross", "alpaca", "ant", "ape", "badger", "bat", "bear", "beaver", "bee",
    "beetle", "bison", "bittern", "boar", "bobcat", "buffalo", "bumblebee", "butterfly", "buzzard",
    "camel", "capybara", "caribou", "cat", "caterpillar", "chaffinch", "chamois", "cheetah",
    "chickadee", "chimpanzee", "chipmunk", "cicada", "cobra", "condor", "coot", "cormorant",
    "cougar", "coyote", "crab", "crane", "crayfish", "cricket", "crocodile", "crow", "curlew",
    "cuttlefish", "deer", "dingo", "dog", "dolphin", "donkey", "dove", "dragonfly", "duck",
    "eagle", "eel", "egret", "elk", "emu", "falcon", "ferret", "finch", "firefly", "flamingo",
    "fly", "fox", "frog", "gazelle", "gecko", "gnat", "goat", "goldfinch", "goose", "gopher",
    "gorilla", "goshawk", "grasshopper", "grebe", "grouse", "guillemot", "gull", "haddock",
    "hamster", "hare", "harrier", "hawk", "hedgehog", "heron", "herring", "hippo", "hornet",
    "horse", "hound", "hummingbird", "hyena", "ibex", "ibis", "iguana", "jackal", "jackdaw",
    "jaguar", "jay", "jellyfish", "kangaroo", "kestrel", "kingfisher", "kite", "kiwi", "koala",
    "ladybird", "lamprey", "lapwing", "lemming", "lemur", "leopard", "limpet", "lion", "lizard",
    "llama", "lobster", "locust", "loon", "lynx", "macaw", "magpie", "mallard", "mamba",
    "manatee", "manta", "marmot", "marten", "martin", "mastiff", "mayfly", "meerkat", "merganser",
    "mink", "minnow", "mockingbird", "mole", "mongoose", "monkey", "moose", "moth", "mouse",
    "mule", "mussel", "newt", "nightjar", "ocelot", "octopus", "opossum", "orca", "oriole",
    "osprey", "ostrich", "otter", "owl", "ox", "oyster", "panda", "panther", "parrot",
    "partridge", "peacock", "pelican", "penguin", "perch", "petrel", "pheasant", "pigeon", "pike",
    "plover", "pony", "porcupine", "porpoise", "possum", "prawn", "puffin", "puma", "python",
    "quail", "quokka", "rabbit", "raccoon", "ram", "raven", "ray", "reindeer", "rhino", "robin",
    "rook", "rooster", "salmon", "sandpiper", "sardine", "seagull", "seal", "serval", "shark",
    "sheep", "shrew", "shrimp", "skunk", "sloth", "slug", "snail", "snake", "snipe", "sparrow",
    "spider", "squid", "squirrel", "starling", "stoat", "stork", "sturgeon", "swallow", "swan",
    "swift", "tadpole", "tapir", "tern", "thrush", "tick", "tiger", "toad", "toucan", "trout",
    "tuna", "turtle", "viper", "vole", "vulture", "wallaby", "walrus", "warbler", "wasp",
    "weasel", "whale", "whippet", "wildebeest", "wolf", "wombat", "woodpecker", "worm", "wren",
    "yak", "zebra",
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

/// The longest display name Flit will announce.
pub const MAX_NAME_CHARS: usize = 32;

/// A fresh display name: two words, e.g. `amber heron`.
///
/// A starting point for a first run, not an identity. The name is stored and
/// the person may change it, so it carries no proof; the key does.
pub fn random_name() -> Result<String, CryptoError> {
    let mut bytes = [0u8; 4];
    getrandom::getrandom(&mut bytes)?;
    Ok(name_from_bytes(bytes))
}

/// Pick a name from four bytes of randomness.
fn name_from_bytes(bytes: [u8; 4]) -> String {
    let qualifier = NAME_QUALIFIERS
        [usize::from(u16::from_be_bytes([bytes[0], bytes[1]])) % NAME_QUALIFIERS.len()];
    let creature = NAME_CREATURES
        [usize::from(u16::from_be_bytes([bytes[2], bytes[3]])) % NAME_CREATURES.len()];
    format!("{qualifier} {creature}")
}

/// Clean a name someone typed: trim the ends, collapse runs of spaces, and
/// refuse the empty string or anything longer than [`MAX_NAME_CHARS`].
///
/// Returns `None` when nothing usable is left, so callers keep the old name
/// instead of announcing a blank one.
#[must_use]
pub fn clean_name(raw: &str) -> Option<String> {
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return None;
    }
    Some(name)
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
    fn a_generated_name_is_two_words_from_the_lists() {
        for seed in 0u8..64 {
            let name = name_from_bytes([seed, seed.wrapping_mul(7), seed.wrapping_add(3), seed]);
            let parts: Vec<&str> = name.split(' ').collect();
            assert_eq!(parts.len(), 2, "{name}");
            assert!(NAME_QUALIFIERS.contains(&parts[0]), "{name}");
            assert!(NAME_CREATURES.contains(&parts[1]), "{name}");
        }
        let fresh = random_name().unwrap();
        assert_eq!(fresh.split(' ').count(), 2, "{fresh}");
    }

    #[test]
    fn a_typed_name_is_cleaned_before_it_is_used() {
        assert_eq!(clean_name("  amber   heron "), Some("amber heron".into()));
        assert_eq!(clean_name("Rhea's laptop"), Some("Rhea's laptop".into()));
        assert_eq!(clean_name(" \t\n "), None);
        assert_eq!(clean_name(&"a".repeat(MAX_NAME_CHARS + 1)), None);
        assert!(clean_name(&"a".repeat(MAX_NAME_CHARS)).is_some());
    }

    #[test]
    fn the_word_lists_are_long_enough_and_clean() {
        for list in [NAME_QUALIFIERS, NAME_CREATURES] {
            assert!(list.len() >= 200, "only {} words", list.len());
            let unique: std::collections::HashSet<_> = list.iter().collect();
            assert_eq!(unique.len(), list.len(), "a word repeats");
            assert!(
                list.iter()
                    .all(|word| word.chars().all(|c| c.is_ascii_lowercase() || c == ' ')),
                "words are lowercase"
            );
        }
    }

    #[test]
    fn debug_never_leaks_the_secret() {
        let text = format!("{:?}", fixed());
        assert!(text.contains("device_id"));
        assert!(!text.contains("secret"));
    }
}
