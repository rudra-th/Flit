//! The announce datagram: what goes in it, what comes out of it, and the
//! limits that make a hostile datagram cheap to reject.
//!
//! The announce is deliberately the smallest payload in the protocol. It
//! carries a display name, a device class, a public fingerprint and the port
//! the control plane is on. It never carries a file name, a size, a history or
//! anything that would let a passive listener learn what is being sent.
//!
//! Everything received on the discovery port is hostile input. Decoding
//! therefore runs the same validation as encoding, so a peer cannot become a
//! peer by sending a value this build would refuse to send itself.

use flit_api::{Announce, Info, PROTO_VERSION};
use thiserror::Error;

/// Largest datagram that will be parsed.
///
/// A legal announce is comfortably under 512 bytes even with sixteen
/// capabilities; the extra headroom keeps the limit from becoming a protocol
/// change when a capability is added. Anything larger is dropped before the
/// JSON parser sees it.
pub const MAX_ANNOUNCE_BYTES: usize = 1024;

/// Longest display name, in characters.
///
/// Long enough for "Rudra's second laptop", short enough that a device card
/// never needs to scroll and a hostile peer cannot fill a log line.
pub const MAX_ALIAS_CHARS: usize = 64;

/// Most capability tokens accepted in one announce.
pub const MAX_CAPS: usize = 16;

/// Longest capability token, in characters.
pub const MAX_CAP_CHARS: usize = 24;

/// Why an announce could not be encoded or decoded.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnnounceError {
    /// The datagram is larger than [`MAX_ANNOUNCE_BYTES`].
    #[error("announce is {size} bytes, limit is {max}")]
    TooLarge {
        /// Encoded size.
        size: usize,
        /// The limit.
        max: usize,
    },
    /// The bytes are not a JSON object with the expected shape.
    #[error("announce is not valid JSON: {0}")]
    Malformed(String),
    /// The peer speaks a different major version.
    #[error("the peer speaks protocol {theirs}, this build speaks {ours}")]
    Unsupported {
        /// What the peer claimed.
        theirs: u16,
        /// What we speak.
        ours: u16,
    },
    /// The fingerprint was not 64 lowercase hex characters.
    #[error("announce fingerprint is not a SHA-256: {0:?}")]
    BadFingerprint(String),
    /// The device id did not match the fingerprint it claims to come from.
    #[error("announce device id {device_id:?} does not match fingerprint {fingerprint:?}")]
    BadDeviceId {
        /// The claimed short id.
        device_id: String,
        /// The fingerprint the id should have been derived from.
        fingerprint: String,
    },
    /// The control port was zero.
    #[error("announce port is zero")]
    BadPort,
    /// The display name was empty, too long, or contained control characters.
    #[error("announce alias {0:?} is not a usable display name")]
    BadAlias(String),
    /// A capability token was empty, too long, or contained stray characters.
    #[error("announce capability {0:?} is not a valid token")]
    BadCap(String),
    /// More capability tokens than [`MAX_CAPS`].
    #[error("announce carries {count} capabilities, limit is {max}")]
    TooManyCaps {
        /// How many were offered.
        count: usize,
        /// The limit.
        max: usize,
    },
}

/// Build the announce that describes `info`.
///
/// The announce is a strict subset of `/api/v1/info`, so this is the only
/// place that decides which fields a passive listener gets to see.
#[must_use]
pub fn announce_for(info: &Info) -> Announce {
    Announce {
        proto: info.proto,
        alias: info.alias.clone(),
        device_id: info.device_id.clone(),
        fingerprint: info.fingerprint.clone(),
        device_type: info.device_type,
        port: info.port,
        caps: info.caps.clone(),
        pin_required: info.pin_required,
    }
}

/// Encode an announce for the wire.
///
/// The result is validated with the same rules a receiver applies, so two
/// builds that can decode each other's announces can also send them.
pub fn encode(announce: &Announce) -> Result<Vec<u8>, AnnounceError> {
    let announce = canonical(announce.clone())?;
    let bytes =
        serde_json::to_vec(&announce).map_err(|e| AnnounceError::Malformed(e.to_string()))?;
    if bytes.len() > MAX_ANNOUNCE_BYTES {
        return Err(AnnounceError::TooLarge {
            size: bytes.len(),
            max: MAX_ANNOUNCE_BYTES,
        });
    }
    Ok(bytes)
}

/// Decode an announce received on the discovery port.
///
/// Returns a canonical value: the fingerprint is lowercase, the device id is
/// derived from it, the alias is trimmed, and capabilities are deduplicated.
pub fn decode(datagram: &[u8]) -> Result<Announce, AnnounceError> {
    if datagram.len() > MAX_ANNOUNCE_BYTES {
        return Err(AnnounceError::TooLarge {
            size: datagram.len(),
            max: MAX_ANNOUNCE_BYTES,
        });
    }
    let announce: Announce =
        serde_json::from_slice(datagram).map_err(|e| AnnounceError::Malformed(e.to_string()))?;
    canonical(announce)
}

/// Validate and normalize an announce.
fn canonical(mut announce: Announce) -> Result<Announce, AnnounceError> {
    if announce.proto != PROTO_VERSION {
        return Err(AnnounceError::Unsupported {
            theirs: announce.proto,
            ours: PROTO_VERSION,
        });
    }

    let fingerprint = announce.fingerprint.trim().to_ascii_lowercase();
    let is_hex = fingerprint.len() == 64 && fingerprint.bytes().all(|b| b.is_ascii_hexdigit());
    if !is_hex {
        return Err(AnnounceError::BadFingerprint(announce.fingerprint));
    }

    // The device id is defined as the first 8 bytes of the fingerprint. An
    // announce that disagrees with its own fingerprint is either broken or is
    // trying to be listed under somebody else's short id.
    let expected_id = &fingerprint[..16];
    if !announce.device_id.is_empty() && !announce.device_id.eq_ignore_ascii_case(expected_id) {
        return Err(AnnounceError::BadDeviceId {
            device_id: announce.device_id,
            fingerprint,
        });
    }
    announce.device_id = expected_id.to_string();
    announce.fingerprint = fingerprint;

    if announce.port == 0 {
        return Err(AnnounceError::BadPort);
    }

    announce.alias = announce.alias.trim().to_string();
    let alias_ok = !announce.alias.is_empty()
        && announce.alias.chars().count() <= MAX_ALIAS_CHARS
        && !announce.alias.chars().any(char::is_control);
    if !alias_ok {
        return Err(AnnounceError::BadAlias(announce.alias));
    }

    if announce.caps.len() > MAX_CAPS {
        return Err(AnnounceError::TooManyCaps {
            count: announce.caps.len(),
            max: MAX_CAPS,
        });
    }
    let mut caps: Vec<String> = Vec::with_capacity(announce.caps.len());
    for cap in announce.caps {
        let cap = cap.trim().to_string();
        let valid = !cap.is_empty()
            && cap.chars().count() <= MAX_CAP_CHARS
            && cap
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !valid {
            return Err(AnnounceError::BadCap(cap));
        }
        if !caps.iter().any(|known| known == &cap) {
            caps.push(cap);
        }
    }
    announce.caps = caps;

    Ok(announce)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flit_api::{DeviceType, TLS_PORT};

    fn announce() -> Announce {
        Announce {
            proto: PROTO_VERSION,
            alias: "Rudra's laptop".into(),
            device_id: "0123456789abcdef".into(),
            fingerprint: "0123456789abcdef".to_string() + &"ab".repeat(24),
            device_type: DeviceType::Laptop,
            port: TLS_PORT,
            caps: vec!["resume".into(), "verify".into()],
            pin_required: false,
        }
    }

    #[test]
    fn an_announce_round_trips() {
        let original = announce();
        let bytes = encode(&original).unwrap();
        assert!(bytes.len() <= MAX_ANNOUNCE_BYTES);
        assert_eq!(decode(&bytes).unwrap(), original);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let mut value = serde_json::to_value(announce()).unwrap();
        value["a_field_from_the_future"] = serde_json::json!({"nested": true});
        let decoded = decode(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(decoded.alias, "Rudra's laptop");
    }

    #[test]
    fn oversized_datagrams_are_rejected_before_parsing() {
        let padded = vec![b'{'; MAX_ANNOUNCE_BYTES + 1];
        assert_eq!(
            decode(&padded),
            Err(AnnounceError::TooLarge {
                size: MAX_ANNOUNCE_BYTES + 1,
                max: MAX_ANNOUNCE_BYTES
            })
        );
    }

    #[test]
    fn a_future_protocol_is_refused_not_half_understood() {
        let mut a = announce();
        a.proto = PROTO_VERSION + 1;
        assert!(matches!(
            encode(&a),
            Err(AnnounceError::Unsupported { theirs, .. }) if theirs == PROTO_VERSION + 1
        ));
    }

    #[test]
    fn a_device_id_that_does_not_match_the_fingerprint_is_refused() {
        let mut a = announce();
        a.device_id = "ffffffffffffffff".into();
        assert!(matches!(encode(&a), Err(AnnounceError::BadDeviceId { .. })));
    }

    #[test]
    fn an_empty_device_id_is_filled_in_from_the_fingerprint() {
        let mut a = announce();
        a.device_id.clear();
        let decoded = decode(&encode(&a).unwrap()).unwrap();
        assert_eq!(decoded.device_id, &decoded.fingerprint[..16]);
    }

    #[test]
    fn fingerprints_are_normalized_to_lowercase() {
        let mut a = announce();
        a.fingerprint = a.fingerprint.to_ascii_uppercase();
        let decoded = decode(&encode(&a).unwrap()).unwrap();
        assert_eq!(
            decoded.fingerprint,
            decoded.fingerprint.to_ascii_lowercase()
        );
        assert_eq!(decoded.fingerprint.len(), 64);
    }

    #[test]
    fn a_fingerprint_that_is_not_a_sha256_is_refused() {
        for bad in [
            String::new(),
            "abc".to_string(),
            "g".repeat(64),
            "a".repeat(63),
            "a".repeat(65),
        ] {
            let mut a = announce();
            a.fingerprint = bad;
            assert!(matches!(encode(&a), Err(AnnounceError::BadFingerprint(_))));
        }
    }

    #[test]
    fn port_zero_is_refused() {
        let mut a = announce();
        a.port = 0;
        assert_eq!(encode(&a), Err(AnnounceError::BadPort));
    }

    #[test]
    fn aliases_are_trimmed_and_bounded() {
        let mut a = announce();
        a.alias = "  Desk  ".into();
        assert!(!encode(&a).unwrap().is_empty());
        assert_eq!(decode(&encode(&a).unwrap()).unwrap().alias, "Desk");

        for bad in [
            String::new(),
            "   ".into(),
            "x".repeat(MAX_ALIAS_CHARS + 1),
            "a\nb".into(),
        ] {
            let mut a = announce();
            a.alias = bad;
            assert!(matches!(encode(&a), Err(AnnounceError::BadAlias(_))));
        }
    }

    #[test]
    fn capability_tokens_are_validated_and_deduplicated() {
        let mut a = announce();
        a.caps = vec!["resume".into(), "resume".into(), " pack ".into()];
        let decoded = decode(&encode(&a).unwrap()).unwrap();
        assert_eq!(decoded.caps, vec!["resume".to_string(), "pack".to_string()]);

        for bad in [
            "".to_string(),
            "has space".to_string(),
            "x".repeat(MAX_CAP_CHARS + 1),
        ] {
            let mut a = announce();
            a.caps = vec![bad];
            assert!(matches!(encode(&a), Err(AnnounceError::BadCap(_))));
        }
    }

    #[test]
    fn too_many_capabilities_are_refused() {
        let mut a = announce();
        a.caps = (0..=MAX_CAPS).map(|i| format!("c{i}")).collect();
        assert!(matches!(encode(&a), Err(AnnounceError::TooManyCaps { .. })));
    }

    #[test]
    fn an_announce_is_built_from_info_without_leaking_more_than_a_name() {
        let info = Info {
            proto: PROTO_VERSION,
            alias: "Desk".into(),
            device_id: "0123456789abcdef".into(),
            fingerprint: "ab".repeat(32),
            device_type: DeviceType::Desktop,
            port: TLS_PORT,
            version: "0.1.0".into(),
            caps: vec!["resume".into()],
            pin_required: true,
        };
        let a = announce_for(&info);
        let json = serde_json::to_string(&a).unwrap();
        assert!(!json.contains("version"));
        assert!(json.contains("pin_required"));
    }
}
