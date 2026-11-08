//! Interface enumeration on Unix, via `getifaddrs`.
//!
//! Linux and Android are the v1 targets; the same call exists on macOS, which
//! is why nothing here is Linux-specific.

use std::{net::Ipv4Addr, ptr};

use libc::{c_char, ifaddrs, sockaddr_in};

use super::Interface;

/// Every up, non-loopback IPv4 interface.
pub(super) fn interfaces() -> Vec<Interface> {
    let mut head: *mut ifaddrs = ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut cursor = head;
    while !cursor.is_null() {
        let entry = unsafe { &*cursor };
        if let Some(interface) = read(entry) {
            out.push(interface);
        }
        cursor = entry.ifa_next;
    }

    unsafe { libc::freeifaddrs(head) };
    out
}

/// Turn one `ifaddrs` entry into an [`Interface`], if it is a usable IPv4 one.
fn read(entry: &ifaddrs) -> Option<Interface> {
    if entry.ifa_addr.is_null() {
        return None;
    }
    let family = unsafe { (*entry.ifa_addr).sa_family } as i32;
    if family != libc::AF_INET {
        return None;
    }

    let addr = unsafe { &*entry.ifa_addr.cast::<sockaddr_in>() };
    let ip = Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr));
    if ip.is_loopback() || ip.is_unspecified() || ip.is_link_local() {
        return None;
    }

    // The netmask is optional in principle. 24 is the honest default: it is
    // what a missing mask most often means, and it is the scan width anyway.
    let prefix_len = if entry.ifa_netmask.is_null() {
        24
    } else {
        let mask = unsafe { &*entry.ifa_netmask.cast::<sockaddr_in>() };
        u32::from_be(mask.sin_addr.s_addr).count_ones() as u8
    };

    let index = if entry.ifa_name.is_null() {
        0
    } else {
        unsafe { libc::if_nametoindex(entry.ifa_name as *const c_char) }
    };

    Some(Interface::new(index, ip, prefix_len))
}
