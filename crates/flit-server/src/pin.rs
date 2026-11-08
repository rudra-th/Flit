//! PIN checks for first contact.
//!
//! A PIN is the short code a receiver shows on its own screen. It travels inside
//! TLS, so it is not encrypted a second time; its job is to stop someone who has
//! already reached the port from opening a session without permission — for
//! example a device on the same guest Wi-Fi, or a stray script pointed at the
//! subnet.
//!
//! Attempts are counted per source address: three failures in a minute and that
//! address waits out the rest of the window, *including* with the right PIN. A
//! legitimate user who fumbles three times waits a minute; an attacker gains
//! nothing by trying. Counting per *device* needs a proven device identity, which
//! arrives with `POST /api/v1/register`; until then the source address is the
//! only thing the receiver actually knows.
//!
//! A missing PIN does not consume an attempt: nothing was guessed. It is still a
//! distinct error (`pin_required`), because a UI that says "wrong PIN" when the
//! user never entered one is lying to them.

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

use flit_api::ErrorCode;

/// Failures allowed per address per window.
pub const MAX_ATTEMPTS: u32 = 3;

/// How long a window lasts, and therefore how long a cooldown is.
pub const WINDOW: Duration = Duration::from_secs(60);

/// Shortest PIN accepted, in digits.
pub const MIN_PIN_DIGITS: usize = 4;

/// Longest PIN accepted, in digits. Longer is not more secure on a screen, and
/// it makes the code painful to read out loud.
pub const MAX_PIN_DIGITS: usize = 12;

/// A PIN that cannot be used as configured.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PinError {
    /// The PIN was empty, too long, or not made of digits.
    #[error("a PIN must be {MIN_PIN_DIGITS} to {MAX_PIN_DIGITS} digits, not {0:?}")]
    NotUsable(String),
}

/// The receiver's PIN policy and its recent failures.
pub struct PinGuard {
    pin: Option<String>,
    attempts: Mutex<HashMap<IpAddr, Attempts>>,
}

/// One address's recent failures.
#[derive(Debug, Clone, Copy)]
struct Attempts {
    count: u32,
    started: Instant,
}

impl PinGuard {
    /// Build a guard, validating the PIN if one was configured.
    ///
    /// An empty string is treated as "no PIN": a user who clears the field wants
    /// an open receiver, not a receiver whose PIN is the empty string.
    pub fn new(pin: Option<String>) -> Result<Self, PinError> {
        let pin = match pin {
            None => None,
            Some(pin) if pin.is_empty() => None,
            Some(pin) => {
                let usable = pin.len() >= MIN_PIN_DIGITS
                    && pin.len() <= MAX_PIN_DIGITS
                    && pin.bytes().all(|byte| byte.is_ascii_digit());
                if !usable {
                    return Err(PinError::NotUsable(pin));
                }
                Some(pin)
            }
        };
        Ok(Self {
            pin,
            attempts: Mutex::new(HashMap::new()),
        })
    }

    /// Whether a PIN is needed at all. Advertised as `pin_required`.
    #[must_use]
    pub fn required(&self) -> bool {
        self.pin.is_some()
    }

    /// Check one request's PIN.
    ///
    /// Returns the protocol error to answer with, or `Ok(())` to continue.
    pub fn check(&self, peer: IpAddr, presented: Option<&str>) -> Result<(), ErrorCode> {
        let Some(expected) = self.pin.as_deref() else {
            return Ok(());
        };

        let mut attempts = self.attempts.lock().expect("poisoned pin lock");
        prune(&mut attempts);

        let entry = attempts.get(&peer).copied().unwrap_or(Attempts {
            count: 0,
            started: Instant::now(),
        });

        if entry.count >= MAX_ATTEMPTS {
            // Locked out. Refuse even a correct PIN: otherwise the limit stops
            // nothing, because the attacker would simply guess until right.
            return Err(ErrorCode::PinIncorrect);
        }

        let Some(presented) = presented else {
            return Err(ErrorCode::PinRequired);
        };

        if same_digits(expected, presented) {
            attempts.remove(&peer);
            return Ok(());
        }

        let count = entry.count + 1;
        attempts.insert(
            peer,
            Attempts {
                count,
                started: entry.started,
            },
        );
        Err(ErrorCode::PinIncorrect)
    }

    /// Forget every recorded failure. Used by tests and by a future "unlock"
    /// action in the desktop UI.
    pub fn forget_all(&self) {
        self.attempts.lock().expect("poisoned pin lock").clear();
    }
}

/// Drop windows that have run out, so the map cannot grow without bound.
fn prune(attempts: &mut HashMap<IpAddr, Attempts>) {
    let now = Instant::now();
    attempts.retain(|_, entry| now.duration_since(entry.started) < WINDOW);
}

/// Compare without an early exit.
///
/// A short PIN compared with `==` leaks its length and matching prefix through
/// timing. That is hard to exploit across a network and a TLS record boundary,
/// but the fix costs nothing, so there is no reason to leave the question open.
fn same_digits(expected: &str, presented: &str) -> bool {
    let expected = expected.as_bytes();
    let presented = presented.as_bytes();
    if expected.len() != presented.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in expected.iter().zip(presented) {
        difference |= left ^ right;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> IpAddr {
        "192.168.1.50".parse().unwrap()
    }

    fn other() -> IpAddr {
        "192.168.1.51".parse().unwrap()
    }

    fn guarded(pin: &str) -> PinGuard {
        PinGuard::new(Some(pin.to_string())).unwrap()
    }

    #[test]
    fn an_unconfigured_receiver_accepts_anything() {
        let guard = PinGuard::new(None).unwrap();
        assert!(!guard.required());
        assert_eq!(guard.check(peer(), None), Ok(()));
        assert_eq!(guard.check(peer(), Some("000000")), Ok(()));
    }

    #[test]
    fn an_empty_pin_means_open_not_empty_string() {
        let guard = PinGuard::new(Some(String::new())).unwrap();
        assert!(!guard.required());
        assert_eq!(guard.check(peer(), None), Ok(()));
    }

    #[test]
    fn unusable_pins_are_refused_at_configuration() {
        for bad in ["123", "abcdef", "12 345", "1234567890123"] {
            assert!(
                matches!(
                    PinGuard::new(Some(bad.to_string())),
                    Err(PinError::NotUsable(_))
                ),
                "{bad:?}"
            );
        }
        // An empty string is the one exception: it means "no PIN".
        assert!(PinGuard::new(Some(String::new())).is_ok());
        for good in ["1234", "482913", "123456789012"] {
            assert!(PinGuard::new(Some(good.to_string())).is_ok(), "{good:?}");
        }
    }

    #[test]
    fn a_missing_pin_is_a_different_error_from_a_wrong_one() {
        let guard = guarded("482913");
        assert!(guard.required());
        assert_eq!(guard.check(peer(), None), Err(ErrorCode::PinRequired));
        assert_eq!(
            guard.check(peer(), Some("000000")),
            Err(ErrorCode::PinIncorrect)
        );
        assert_eq!(guard.check(peer(), Some("482913")), Ok(()));
    }

    #[test]
    fn a_missing_pin_does_not_consume_an_attempt() {
        let guard = guarded("482913");
        for _ in 0..10 {
            assert_eq!(guard.check(peer(), None), Err(ErrorCode::PinRequired));
        }
        assert_eq!(guard.check(peer(), Some("482913")), Ok(()));
    }

    #[test]
    fn three_wrong_guesses_lock_the_address_out_for_the_window() {
        let guard = guarded("482913");
        for _ in 0..MAX_ATTEMPTS {
            assert_eq!(
                guard.check(peer(), Some("111111")),
                Err(ErrorCode::PinIncorrect)
            );
        }
        // Even the right PIN waits out the window: a limit that lets the
        // attacker keep guessing is not a limit.
        assert_eq!(
            guard.check(peer(), Some("482913")),
            Err(ErrorCode::PinIncorrect)
        );
        // A different address is unaffected.
        assert_eq!(guard.check(other(), Some("482913")), Ok(()));
    }

    #[test]
    fn a_correct_pin_clears_earlier_failures() {
        let guard = guarded("482913");
        assert_eq!(
            guard.check(peer(), Some("111111")),
            Err(ErrorCode::PinIncorrect)
        );
        assert_eq!(
            guard.check(peer(), Some("222222")),
            Err(ErrorCode::PinIncorrect)
        );
        assert_eq!(guard.check(peer(), Some("482913")), Ok(()));
        // The count is back to zero, so two more failures do not lock out.
        for _ in 0..(MAX_ATTEMPTS - 1) {
            assert_eq!(
                guard.check(peer(), Some("333333")),
                Err(ErrorCode::PinIncorrect)
            );
        }
        assert_eq!(guard.check(peer(), Some("482913")), Ok(()));
    }

    #[test]
    fn an_expired_window_starts_over() {
        let guard = guarded("482913");
        for _ in 0..MAX_ATTEMPTS {
            let _ = guard.check(peer(), Some("111111"));
        }
        assert_eq!(
            guard.check(peer(), Some("482913")),
            Err(ErrorCode::PinIncorrect)
        );

        // Age the window past its lifetime instead of sleeping for a minute.
        {
            let mut attempts = guard.attempts.lock().unwrap();
            attempts.insert(
                peer(),
                Attempts {
                    count: MAX_ATTEMPTS,
                    started: Instant::now() - WINDOW - Duration::from_secs(1),
                },
            );
        }

        assert_eq!(guard.check(peer(), Some("482913")), Ok(()));
    }

    #[test]
    fn comparison_does_not_short_circuit_on_length_or_prefix() {
        assert!(same_digits("482913", "482913"));
        assert!(!same_digits("482913", "482912"));
        assert!(!same_digits("482913", "48291"));
        assert!(!same_digits("482913", ""));
        assert!(!same_digits("", "0"));
        assert!(same_digits("", ""));
    }
}
