//! Which of the relay's addresses go into a relay code.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Reachable from the internet: not loopback, link-local, private, CGNAT,
/// unique-local, documentation or otherwise reserved.
pub fn is_public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(&v4);
            }
            let s = v6.segments();
            // Global unicast is 2000::/3; minus documentation 2001:db8::/32.
            (s[0] & 0xe000) == 0x2000 && !(s[0] == 0x2001 && s[1] == 0x0db8)
        }
    }
}

fn is_public_v4(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        // 100.64.0.0/10 carrier-grade NAT (also Tailscale).
        || (o[0] == 100 && (o[1] & 0xc0) == 64)
        // 192.0.0.0/24 protocol assignments, 198.18.0.0/15 benchmarking,
        // 240.0.0.0/4 reserved.
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 198 && (o[1] & 0xfe) == 18)
        || o[0] >= 240)
}

/// Public IPs of the local network interfaces. Sorted, so the relay code
/// doesn't change between starts.
pub fn interface_ips() -> Vec<IpAddr> {
    let mut ips: Vec<IpAddr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|i| i.ip())
        .filter(is_public)
        .collect();
    ips.sort_by_key(|ip| (ip.is_ipv6(), *ip));
    ips.dedup();
    ips
}

/// The IPs for a relay code: the configured ones verbatim, else the public
/// interface IPs plus any public IP clients observed us at.
pub fn advertised(configured: &[IpAddr], observed: &[IpAddr]) -> Vec<IpAddr> {
    if !configured.is_empty() {
        return configured.to_vec();
    }
    let mut ips = interface_ips();
    for ip in observed {
        if is_public(ip) && !ips.contains(ip) {
            ips.push(*ip);
        }
    }
    ips.sort_by_key(|ip| (ip.is_ipv6(), *ip));
    ips
}

pub fn unspecified(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => *v4 == Ipv4Addr::UNSPECIFIED,
        IpAddr::V6(v6) => *v6 == Ipv6Addr::UNSPECIFIED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_filter() {
        for ip in ["203.0.114.7", "8.8.8.8", "2a01:4f8::1", "::ffff:8.8.8.8"] {
            assert!(is_public(&ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "100.127.255.1",
            "0.0.0.0",
            "203.0.113.7",
            "240.0.0.1",
            "::1",
            "fe80::1",
            "fd00::1",
            "2001:db8::1",
            "::",
            "::ffff:10.0.0.1",
        ] {
            assert!(!is_public(&ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn configured_addrs_win_verbatim() {
        let cfg: Vec<IpAddr> = vec!["10.0.0.5".parse().unwrap()];
        assert_eq!(advertised(&cfg, &["8.8.8.8".parse().unwrap()]), cfg);
    }
}
