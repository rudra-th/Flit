//! Interface enumeration on Windows, via `GetAdaptersAddresses`.
//!
//! `windows-sys` is only used here, so the unsafe surface of the crate is one
//! function and one struct walk. Everything else works on owned values.

use std::{mem::size_of, net::Ipv4Addr, ptr};

use windows_sys::Win32::{
    Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR},
    NetworkManagement::{
        IpHelper::{
            GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
            GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
        },
        Ndis::IfOperStatusUp,
    },
    Networking::WinSock::{AF_INET, SOCKADDR_IN, SOCKET_ADDRESS},
};

use super::Interface;

/// Every up, non-loopback IPv4 interface.
pub(super) fn interfaces() -> Vec<Interface> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;

    // A first guess of 16 KiB covers a household machine; the loop exists for
    // the machine with thirty adapters, where Windows answers with the size it
    // actually needs.
    let mut size: u32 = 16 * 1024;
    for _ in 0..4 {
        // `IP_ADAPTER_ADDRESSES_LH` contains u64s. Allocating a byte buffer and
        // casting it would be a misaligned pointer; allocating u64 words is
        // not.
        let mut buffer = vec![0u64; (size as usize).div_ceil(size_of::<u64>())];
        let result = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_INET),
                flags,
                ptr::null(),
                buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                &mut size,
            )
        };

        if result == ERROR_BUFFER_OVERFLOW {
            continue;
        }
        if result != NO_ERROR {
            return Vec::new();
        }

        return unsafe { walk(buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>()) };
    }
    Vec::new()
}

/// Walk the adapter list, collecting addresses of interfaces that are up.
///
/// # Safety
///
/// `head` must point at a buffer returned by `GetAdaptersAddresses` that is
/// still alive, with every `Next` pointer inside that buffer.
unsafe fn walk(head: *const IP_ADAPTER_ADDRESSES_LH) -> Vec<Interface> {
    let mut out = Vec::new();
    let mut adapter = head;
    while !adapter.is_null() {
        let entry = unsafe { &*adapter };
        if entry.OperStatus == IfOperStatusUp {
            let index = unsafe { entry.Anonymous1.Anonymous.IfIndex };
            let mut unicast = entry.FirstUnicastAddress;
            while !unicast.is_null() {
                let address = unsafe { &*unicast };
                if let Some(ip) = ipv4(&address.Address) {
                    if !ip.is_loopback() && !ip.is_unspecified() && !ip.is_link_local() {
                        out.push(Interface::new(index, ip, address.OnLinkPrefixLength));
                    }
                }
                unicast = address.Next;
            }
        }
        adapter = entry.Next;
    }
    out
}

/// Read a `SOCKADDR_IN` out of a `SOCKET_ADDRESS`, if it is IPv4 at all.
fn ipv4(address: &SOCKET_ADDRESS) -> Option<Ipv4Addr> {
    if address.lpSockaddr.is_null() || address.iSockaddrLength < size_of::<SOCKADDR_IN>() as i32 {
        return None;
    }
    let raw = unsafe { &*address.lpSockaddr.cast::<SOCKADDR_IN>() };
    if raw.sin_family != AF_INET {
        return None;
    }
    // `S_addr` is the address in network byte order held in a native integer.
    Some(Ipv4Addr::from(u32::from_be(unsafe {
        raw.sin_addr.S_un.S_addr
    })))
}
