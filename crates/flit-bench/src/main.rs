//! Reproducible benchmark harness.
//!
//! Every number published in `docs/BENCHMARKS.md` comes from this binary, with
//! the hardware and link type recorded alongside it. Loopback results are
//! labelled as loopback and are never used as a product claim.
//!
//! M0 ships the skeleton and the reporting format only.

#![deny(missing_docs)]

/// A single measured result, with enough context to reproduce it.
#[derive(Debug, Clone)]
pub struct Measurement {
    /// What was measured, e.g. `single-file-put`.
    pub name: &'static str,
    /// Transfer size in bytes.
    pub bytes: u64,
    /// Elapsed time in seconds.
    pub seconds: f64,
    /// Where the traffic ran, e.g. `loopback` or `wifi6-5ghz`.
    pub link: &'static str,
}

impl Measurement {
    /// Throughput in mebibytes per second.
    #[must_use]
    pub fn mib_per_second(&self) -> f64 {
        if self.seconds <= 0.0 {
            return 0.0;
        }
        (self.bytes as f64 / (1024.0 * 1024.0)) / self.seconds
    }
}

fn main() {
    println!(
        "flit-bench {} (protocol {})",
        env!("CARGO_PKG_VERSION"),
        flit_api::PROTO_VERSION
    );
    println!("No scenarios registered yet; transport benchmarks land with M1.");
}
