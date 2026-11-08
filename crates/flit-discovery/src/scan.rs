//! The subnet scan: the fallback that makes blocked multicast a non-event.
//!
//! A scan dials every candidate address over TLS and asks `GET /api/v1/info`.
//! The certificate does not need a pin, because this is the first contact that
//! would have supplied one: the connection records the key the peer presented,
//! and the fingerprint in `/info` must match that key. A peer can therefore
//! introduce itself, but it cannot introduce itself as somebody else.
//!
//! Cost is the design constraint. Every host gets a short timeout, every scan
//! has a global budget, and concurrency is capped so a scan never looks like a
//! SYN flood from the receiving side.

use std::{
    collections::HashSet,
    net::Ipv4Addr,
    time::{Duration, Instant},
};

use flit_client::{Probe, probe};
use futures_util::{StreamExt, stream};

/// How many dials may be in flight at once.
pub const CONCURRENCY: usize = 32;

/// How long a single host gets to complete TCP, TLS and `/info`.
///
/// A host that has to be scanned is a host whose multicast did not arrive, and
/// a device that cannot answer in 300 ms is not the device the user is looking
/// for.
pub const PER_HOST: Duration = Duration::from_millis(300);

/// How long a whole scan may take before unanswered hosts are abandoned.
pub const BUDGET: Duration = Duration::from_secs(10);

/// The largest number of addresses one scan will try.
pub const MAX_TARGETS: usize = 1024;

/// How a scan is carried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanOptions {
    /// Parallel dials.
    pub concurrency: usize,
    /// Per-host deadline for TCP, TLS and `/info`.
    pub per_host: Duration,
    /// Whole-scan deadline.
    pub budget: Duration,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            concurrency: CONCURRENCY,
            per_host: PER_HOST,
            budget: BUDGET,
        }
    }
}

/// What a scan found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// Addresses the scan intended to try.
    pub targets: usize,
    /// Addresses that were actually dialled before the budget ran out.
    pub probed: usize,
    /// Devices that answered.
    pub found: Vec<Probe>,
}

impl ScanReport {
    /// Whether the scan ran out of time with hosts left untried.
    #[must_use]
    pub fn incomplete(&self) -> bool {
        self.probed < self.targets
    }
}

/// Dial every address in `targets` and collect the devices that answer.
///
/// `own_id` is filtered out: a scan of a loopback or a VPN can reach this
/// machine, and listing yourself as a nearby device is never useful.
pub async fn scan(
    targets: Vec<Ipv4Addr>,
    port: u16,
    own_id: &str,
    options: &ScanOptions,
) -> ScanReport {
    let started = Instant::now();
    let total = targets.len();
    let concurrency = options.concurrency.max(1);
    let per_host = options.per_host;

    let mut report = ScanReport {
        targets: total,
        ..ScanReport::default()
    };

    let dials = stream::iter(targets.into_iter().map(|ip| {
        let addr = std::net::SocketAddr::from((ip, port));
        async move { (ip, probe(addr, per_host).await) }
    }))
    .buffer_unordered(concurrency);

    futures_util::pin_mut!(dials);
    let mut seen: HashSet<String> = HashSet::new();

    loop {
        let elapsed = started.elapsed();
        if elapsed >= options.budget {
            break;
        }
        let remaining = options.budget - elapsed;

        match tokio::time::timeout(remaining, dials.next()).await {
            Ok(Some((_ip, Ok(found)))) => {
                report.probed += 1;
                if found.info.device_id == own_id {
                    continue;
                }
                if seen.insert(found.info.device_id.clone()) {
                    report.found.push(found);
                }
            }
            Ok(Some((_ip, Err(_)))) => {
                report.probed += 1;
            }
            // The stream ended.
            Ok(None) => break,
            // The budget ran out while waiting for the next answer.
            Err(_) => break,
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_ones_in_the_plan() {
        let options = ScanOptions::default();
        assert_eq!(options.concurrency, 32);
        assert_eq!(options.per_host, Duration::from_millis(300));
    }

    #[test]
    fn a_report_knows_when_it_stopped_early() {
        let mut report = ScanReport {
            targets: 254,
            probed: 254,
            found: Vec::new(),
        };
        assert!(!report.incomplete());
        report.probed = 200;
        assert!(report.incomplete());
    }

    #[tokio::test]
    async fn an_empty_scan_completes_without_touching_the_network() {
        let report = scan(Vec::new(), 47_821, "self", &ScanOptions::default()).await;
        assert_eq!(report.targets, 0);
        assert_eq!(report.probed, 0);
        assert!(report.found.is_empty());
    }
}
