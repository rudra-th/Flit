//! Local network interfaces, and the subnets worth scanning.
//!
//! Discovery needs both halves of this:
//!
//! * **per-interface sockets**, so a laptop on Wi-Fi with a VPN up announces
//!   on both instead of only whichever one owns the default route;
//! * **a subnet to scan**, which is the fallback when multicast is filtered.
//!
//! The plan's rule is a /24 never larger: a machine with a /16 is scanned as
//! the /24 it actually sits in, because 65,534 TLS handshakes is not a
//! discovery mechanism, it is a port scan. The prefix is clamped, not trusted.
//!
//! Only private and carrier-grade-NAT space is scanned. A host with a public
//! address must not touch 254 strangers because it opened a file picker.

use std::net::Ipv4Addr;

/// One IPv4 interface that can carry discovery traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Interface {
    index: u32,
    addr: Ipv4Addr,
    prefix_len: u8,
}

impl Interface {
    /// Build an interface description. `prefix_len` is clamped to 32.
    #[must_use]
    pub const fn new(index: u32, addr: Ipv4Addr, prefix_len: u8) -> Self {
        Self {
            index,
            addr,
            prefix_len: if prefix_len > 32 { 32 } else { prefix_len },
        }
    }

    /// The operating system's interface index, used for socket options.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// The interface's own address.
    #[must_use]
    pub const fn addr(self) -> Ipv4Addr {
        self.addr
    }

    /// The prefix length as reported by the operating system.
    #[must_use]
    pub const fn prefix_len(self) -> u8 {
        self.prefix_len
    }

    /// The /24 this interface sits in, after clamping.
    #[must_use]
    pub fn subnet(self) -> Ipv4Addr {
        network_of(self.addr, self.prefix_len.max(24))
    }
}

/// Every usable IPv4 interface on this machine.
///
/// This is best-effort by design: a machine with no interfaces still gets an
/// empty list, and discovery falls back to the kernel's default route instead
/// of failing to start.
#[must_use]
pub fn interfaces() -> Vec<Interface> {
    #[cfg(windows)]
    {
        windows::interfaces()
    }
    #[cfg(unix)]
    {
        unix::interfaces()
    }
    #[cfg(not(any(windows, unix)))]
    {
        Vec::new()
    }
}

/// The addresses to try in a subnet scan.
///
/// Skips addresses this machine owns, skips subnets that are not private, and
/// stops at `cap` addresses so a machine with four interfaces cannot turn one
/// scan into a thousand connections.
#[must_use]
pub fn scan_targets(interfaces: &[Interface], local: &[Ipv4Addr], cap: usize) -> Vec<Ipv4Addr> {
    let mut targets: Vec<Ipv4Addr> = Vec::new();
    for interface in interfaces {
        let network = interface.subnet();
        if !is_private_space(network) {
            continue;
        }
        for host in hosts_in(network, 24) {
            if local.contains(&host) || targets.contains(&host) {
                continue;
            }
            if targets.len() >= cap {
                return targets;
            }
            targets.push(host);
        }
    }
    targets
}

/// The usable host addresses of `network/prefix`, clamped to a /24.
///
/// Network and broadcast addresses are excluded: one is not a host and the
/// other goes to every host, which is not what a TLS handshake wants.
#[must_use]
pub fn hosts_in(network: Ipv4Addr, prefix_len: u8) -> Vec<Ipv4Addr> {
    let effective = prefix_len.clamp(24, 32);
    let base = network_of(network, effective);
    let size = 1u32 << (32 - u32::from(effective));
    (1..size - 1)
        .map(|i| Ipv4Addr::from(u32::from(base) + i))
        .collect()
}

/// Whether `network` is private, carrier-grade NAT, or otherwise a LAN.
#[must_use]
pub fn is_private_space(network: Ipv4Addr) -> bool {
    let [a, b, ..] = network.octets();
    a == 10
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 100 && (64..=127).contains(&b))
}

fn network_of(addr: Ipv4Addr, prefix_len: u8) -> Ipv4Addr {
    let prefix = prefix_len.min(32);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix))
    };
    Ipv4Addr::from(u32::from(addr) & mask)
}

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slash_24_has_254_usable_hosts() {
        let hosts = hosts_in(Ipv4Addr::new(192, 168, 1, 0), 24);
        assert_eq!(hosts.len(), 254);
        assert_eq!(hosts[0], Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(hosts[253], Ipv4Addr::new(192, 168, 1, 254));
    }

    #[test]
    fn a_slash_16_is_clamped_to_the_slash_24_it_sits_in() {
        let hosts = hosts_in(Ipv4Addr::new(10, 4, 9, 200), 16);
        assert_eq!(hosts.len(), 254);
        assert_eq!(hosts[0], Ipv4Addr::new(10, 4, 9, 1));
    }

    #[test]
    fn a_slash_30_has_two_hosts() {
        let hosts = hosts_in(Ipv4Addr::new(10, 0, 0, 4), 30);
        assert_eq!(
            hosts,
            vec![Ipv4Addr::new(10, 0, 0, 5), Ipv4Addr::new(10, 0, 0, 6)]
        );
    }

    #[test]
    fn tiny_subnets_do_not_produce_a_host() {
        assert!(hosts_in(Ipv4Addr::new(10, 0, 0, 4), 31).is_empty());
        assert!(hosts_in(Ipv4Addr::new(10, 0, 0, 4), 32).is_empty());
    }

    #[test]
    fn only_private_space_is_scanned() {
        assert!(is_private_space(Ipv4Addr::new(192, 168, 4, 0)));
        assert!(is_private_space(Ipv4Addr::new(10, 0, 0, 0)));
        assert!(is_private_space(Ipv4Addr::new(172, 20, 0, 0)));
        assert!(is_private_space(Ipv4Addr::new(100, 64, 0, 0)));
        assert!(!is_private_space(Ipv4Addr::new(172, 32, 0, 0)));
        assert!(!is_private_space(Ipv4Addr::new(8, 8, 8, 0)));
        assert!(!is_private_space(Ipv4Addr::new(169, 254, 1, 0)));
    }

    #[test]
    fn scan_targets_skip_the_local_address_and_cap_the_total() {
        let eth = Interface::new(1, Ipv4Addr::new(192, 168, 1, 7), 24);
        let wifi = Interface::new(2, Ipv4Addr::new(192, 168, 1, 8), 24);
        let vpn = Interface::new(3, Ipv4Addr::new(10, 8, 0, 3), 24);
        let public = Interface::new(4, Ipv4Addr::new(203, 0, 113, 4), 24);

        let targets = scan_targets(
            &[eth, wifi, vpn, public],
            &[Ipv4Addr::new(192, 168, 1, 7)],
            1000,
        );
        assert!(!targets.contains(&Ipv4Addr::new(192, 168, 1, 7)));
        assert!(!targets.contains(&Ipv4Addr::new(203, 0, 113, 1)));
        assert!(!targets.contains(&Ipv4Addr::new(203, 0, 113, 2)));
        // The two interfaces that share a subnet contribute it once.
        assert_eq!(
            targets
                .iter()
                .filter(|ip| ip.octets()[..3] == [192, 168, 1])
                .count(),
            253
        );
        assert!(targets.contains(&Ipv4Addr::new(10, 8, 0, 1)));

        let capped = scan_targets(&[eth, vpn], &[Ipv4Addr::new(192, 168, 1, 7)], 10);
        assert_eq!(capped.len(), 10);
    }

    #[test]
    fn prefix_lengths_are_clamped_by_the_constructor() {
        assert_eq!(Interface::new(1, Ipv4Addr::LOCALHOST, 99).prefix_len(), 32);
        assert_eq!(
            Interface::new(1, Ipv4Addr::new(10, 1, 2, 3), 16).subnet(),
            Ipv4Addr::new(10, 1, 2, 0)
        );
    }
}
