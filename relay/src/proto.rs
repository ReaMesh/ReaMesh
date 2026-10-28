//! The relay code and the `/reamesh/relay-auth/1` exchange, the two things a
//! relay and ReaMesh (`rust/src/relay.rs`) must agree on byte for byte. Kept
//! free of the rest of the relay (libp2p, serde, blake3, hex and anyhow only)
//! so both can build it from one source.
//!
//! Relay code:
//! `reamesh://relay/<relay-peer-id>?a=<ip>,<ip>&d=<dns>&t=<tcp>&q=<quic>&k=<hex key>&n=<label>`
//!
//! Auth: a host proves it holds an access key before the relay accepts its
//! reservation. The proof binds the key to both ends of the authenticated
//! connection, so a proof sniffed or replayed on another connection is
//! worthless:
//! `blake3::keyed_hash(key, b"reamesh-relay-auth/1" || relay_peer_id || client_peer_id)`.

use std::fmt::Write as _;
use std::net::IpAddr;

use anyhow::{Context, bail};
use libp2p::{Multiaddr, PeerId, StreamProtocol, multiaddr::Protocol};
use serde::{Deserialize, Serialize};

pub const SCHEME_PREFIX: &str = "reamesh://relay/";

pub type AccessKey = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayCode {
    pub peer_id: PeerId,
    /// Public IPs only: a private one would send a host off to a machine on
    /// its own network.
    pub ips: Vec<IpAddr>,
    /// Optional DNS name, dialed as `/dns/<name>/…`.
    pub dns: Option<String>,
    pub tcp_port: u16,
    pub quic_port: u16,
    pub key: AccessKey,
    /// The label of the access key, for display.
    pub label: String,
}

impl RelayCode {
    pub fn encode(&self) -> String {
        let mut s = format!("{SCHEME_PREFIX}{}?", self.peer_id);
        if !self.ips.is_empty() {
            let ips: Vec<String> = self.ips.iter().map(IpAddr::to_string).collect();
            let _ = write!(s, "a={}&", ips.join(","));
        }
        if let Some(d) = &self.dns {
            let _ = write!(s, "d={}&", percent_encode(d));
        }
        let _ = write!(
            s,
            "t={}&q={}&k={}&n={}",
            self.tcp_port,
            self.quic_port,
            hex::encode(self.key),
            percent_encode(&self.label)
        );
        s
    }

    pub fn parse(code: &str) -> anyhow::Result<Self> {
        let rest = code
            .trim()
            .strip_prefix(SCHEME_PREFIX)
            .context("not a reamesh://relay/ code")?;
        let (id, query) = rest
            .split_once('?')
            .context("relay code has no parameters")?;
        let peer_id: PeerId = id
            .trim_end_matches('/')
            .parse()
            .context("bad relay peer id")?;
        let (mut ips, mut dns, mut tcp, mut quic, mut key, mut label) =
            (Vec::new(), None, None, None, None, String::new());
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = percent_decode(v)?;
            match k {
                "a" => {
                    for ip in v.split(',').filter(|s| !s.is_empty()) {
                        ips.push(ip.parse().with_context(|| format!("bad relay ip {ip:?}"))?);
                    }
                }
                "d" if !v.is_empty() => dns = Some(v),
                "t" => tcp = Some(v.parse().context("bad tcp port")?),
                "q" => quic = Some(v.parse().context("bad quic port")?),
                "k" => {
                    let bytes = hex::decode(&v).context("access key is not hex")?;
                    key = Some(
                        AccessKey::try_from(bytes.as_slice())
                            .ok()
                            .context("access key must be 32 bytes")?,
                    );
                }
                "n" => label = v,
                // Unknown parameters are ignored so the format can grow.
                _ => {}
            }
        }
        if ips.is_empty() && dns.is_none() {
            bail!("relay code has neither an address (a=) nor a DNS name (d=)");
        }
        Ok(Self {
            peer_id,
            ips,
            dns,
            tcp_port: tcp.context("relay code has no tcp port (t=)")?,
            quic_port: quic.context("relay code has no quic port (q=)")?,
            key: key.context("relay code has no access key (k=)")?,
            label,
        })
    }

    /// Every dialable address: TCP and QUIC per IP and per DNS name, each
    /// ending in `/p2p/<relay-peer-id>`.
    pub fn multiaddrs(&self) -> Vec<Multiaddr> {
        let mut hosts: Vec<Protocol<'static>> = self
            .ips
            .iter()
            .map(|ip| match ip {
                IpAddr::V4(v4) => Protocol::Ip4(*v4),
                IpAddr::V6(v6) => Protocol::Ip6(*v6),
            })
            .collect();
        if let Some(d) = &self.dns {
            hosts.push(Protocol::Dns(d.clone().into()));
        }
        let mut out = Vec::new();
        for host in hosts {
            let base = Multiaddr::empty().with(host);
            out.push(
                base.clone()
                    .with(Protocol::Tcp(self.tcp_port))
                    .with(Protocol::P2p(self.peer_id)),
            );
            out.push(
                base.with(Protocol::Udp(self.quic_port))
                    .with(Protocol::QuicV1)
                    .with(Protocol::P2p(self.peer_id)),
            );
        }
        out
    }
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn percent_decode(s: &str) -> anyhow::Result<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = s.get(i + 1..i + 3).context("truncated percent escape")?;
                out.push(u8::from_str_radix(hex, 16).context("bad percent escape")?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).context("relay code is not UTF-8")
}

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/reamesh/relay-auth/1");
const DOMAIN: &[u8] = b"reamesh-relay-auth/1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthRequest {
    #[serde(with = "serde_bytes")]
    pub proof: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthResponse {
    pub ok: bool,
    pub reason: Option<String>,
    pub key_label: Option<String>,
}

pub const REASON_BAD_KEY: &str = "bad_key";
pub const REASON_BANNED: &str = "banned";
pub const REASON_FULL: &str = "full";

impl AuthResponse {
    pub fn accepted(label: &str) -> Self {
        Self {
            ok: true,
            reason: None,
            key_label: Some(label.to_owned()),
        }
    }
    pub fn refused(reason: &str) -> Self {
        Self {
            ok: false,
            reason: Some(reason.to_owned()),
            key_label: None,
        }
    }
}

pub fn proof(key: &AccessKey, relay: &PeerId, client: &PeerId) -> [u8; 32] {
    let mut msg = DOMAIN.to_vec();
    msg.extend_from_slice(&relay.to_bytes());
    msg.extend_from_slice(&client.to_bytes());
    *blake3::keyed_hash(key, &msg).as_bytes()
}

/// The label of the key `proof` was made with, if any. Every key is checked,
/// each in constant time (`blake3::Hash` equality), so neither timing nor an
/// early exit reveals which key came close.
pub fn verify<'a>(
    keys: impl IntoIterator<Item = (&'a str, &'a AccessKey)>,
    relay: &PeerId,
    client: &PeerId,
    proof_bytes: &[u8],
) -> Option<&'a str> {
    let given = blake3::Hash::from_bytes(<[u8; 32]>::try_from(proof_bytes).ok()?);
    let mut found = None;
    for (label, key) in keys {
        if blake3::Hash::from_bytes(proof(key, relay, client)) == given && found.is_none() {
            found = Some(label);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::identity::Keypair;

    fn peer() -> PeerId {
        Keypair::generate_ed25519().public().to_peer_id()
    }

    fn sample() -> RelayCode {
        RelayCode {
            peer_id: Keypair::generate_ed25519().public().to_peer_id(),
            ips: vec![
                "203.0.113.7".parse().unwrap(),
                "2001:db8::1".parse().unwrap(),
            ],
            dns: Some("relay.example.org".into()),
            tcp_port: 4001,
            quic_port: 443,
            key: [0xab; 32],
            label: "Bens Band & Co/ü".into(),
        }
    }

    #[test]
    fn round_trip() {
        let code = sample();
        let text = code.encode();
        assert!(text.starts_with("reamesh://relay/12D3"));
        assert!(text.contains("a=203.0.113.7,2001:db8::1&"));
        assert!(text.contains(&format!("k={}", "ab".repeat(32))));
        assert!(text.contains("n=Bens%20Band%20%26%20Co%2F%C3%BC"));
        assert_eq!(RelayCode::parse(&text).unwrap(), code);
    }

    #[test]
    fn round_trip_without_dns_or_ips() {
        let mut code = sample();
        code.dns = None;
        assert_eq!(RelayCode::parse(&code.encode()).unwrap(), code);
        let mut code = sample();
        code.ips.clear();
        assert!(!code.encode().contains("a="));
        assert_eq!(RelayCode::parse(&code.encode()).unwrap(), code);
    }

    #[test]
    fn multiaddrs_expand_every_host() {
        let code = sample();
        let id = code.peer_id;
        let addrs: Vec<String> = code.multiaddrs().iter().map(|a| a.to_string()).collect();
        assert_eq!(
            addrs,
            vec![
                format!("/ip4/203.0.113.7/tcp/4001/p2p/{id}"),
                format!("/ip4/203.0.113.7/udp/443/quic-v1/p2p/{id}"),
                format!("/ip6/2001:db8::1/tcp/4001/p2p/{id}"),
                format!("/ip6/2001:db8::1/udp/443/quic-v1/p2p/{id}"),
                format!("/dns/relay.example.org/tcp/4001/p2p/{id}"),
                format!("/dns/relay.example.org/udp/443/quic-v1/p2p/{id}"),
            ]
        );
    }

    #[test]
    fn rejects_broken_codes() {
        let good = sample().encode();
        assert!(RelayCode::parse("reamesh://join/abc").is_err());
        assert!(RelayCode::parse(&good.replace(&"ab".repeat(32), "abcd")).is_err());
        assert!(RelayCode::parse(&good.replace("t=4001&", "")).is_err());
        let no_host = good
            .replace("a=203.0.113.7,2001:db8::1&", "")
            .replace("d=relay.example.org&", "");
        assert!(RelayCode::parse(&no_host).is_err());
    }

    #[test]
    fn proof_accepts_the_right_key_and_binding() {
        let (relay, client) = (peer(), peer());
        let keys = [
            ("alice".to_owned(), [1u8; 32]),
            ("bob".to_owned(), [2u8; 32]),
        ];
        let iter = || keys.iter().map(|(l, k)| (l.as_str(), k));
        let p = proof(&[2u8; 32], &relay, &client);
        assert_eq!(verify(iter(), &relay, &client, &p), Some("bob"));
    }

    #[test]
    fn proof_rejects_wrong_key() {
        let (relay, client) = (peer(), peer());
        let keys = [("alice".to_owned(), [1u8; 32])];
        let p = proof(&[9u8; 32], &relay, &client);
        assert_eq!(
            verify(
                keys.iter().map(|(l, k)| (l.as_str(), k)),
                &relay,
                &client,
                &p
            ),
            None
        );
    }

    #[test]
    fn proof_rejects_wrong_peer_binding() {
        let (relay, client, other) = (peer(), peer(), peer());
        let keys = [("alice".to_owned(), [1u8; 32])];
        let iter = || keys.iter().map(|(l, k)| (l.as_str(), k));
        // Made for another client: replaying it from `client` fails.
        let p = proof(&[1u8; 32], &relay, &other);
        assert_eq!(verify(iter(), &relay, &client, &p), None);
        // Made for another relay.
        let p = proof(&[1u8; 32], &other, &client);
        assert_eq!(verify(iter(), &relay, &client, &p), None);
        // Wrong length.
        assert_eq!(verify(iter(), &relay, &client, &[0u8; 31]), None);
    }
}
