//! What a ReaMesh peer and a `reamesh-relay` server have to agree on: the
//! relay code format, the addresses it expands to, and the
//! `/reamesh/relay-auth/1` exchange (message types and proof).
//!
//! Its own crate so the core (`rust/`) and the relay server (`relay/`) build
//! it from one source and can't drift apart; it needs only libp2p, serde,
//! serde_bytes and blake3. `app/src/lib/relay.ts` mirrors the parser.
//!
//! Relay code:
//! `reamesh://relay/<relay peer id>?a=<ip>,<ip>&d=<dns name>&t=<tcp>&q=<quic>&k=<key>&n=<label>`
//! with `k` the 32-byte access key as 64 lowercase hex characters and `n` the
//! operator's label, percent-encoded. Each IP and the DNS name expand to
//! `/tcp/<t>` and `/udp/<q>/quic-v1`, followed by `/p2p/<relay peer id>`.
//!
//! Auth (request-response, CBOR): request `{ "proof": bytes(32) }`, response
//! `{ "ok": bool, "reason": string|null, "key_label": string|null }`, reasons
//! `bad_key` | `banned` | `full`. `proof = blake3::keyed_hash(key,
//! "reamesh-relay-auth/1" || relay_peer_id.to_bytes() || own_peer_id.to_bytes())`.
//!
//! Info (`/reamesh/relay-info/1`, request-response, CBOR): what the operator
//! wants key holders to know and how the relay stands, gated by the same proof
//! (see `InfoRequest`, `InfoResponse`). A relay without it answers with
//! "unsupported protocol", which a client takes as "older relay", never as a
//! failure: every field has a default, so both sides can grow.

use std::fmt::Write as _;
use std::net::IpAddr;

use libp2p::multiaddr::Protocol;
use libp2p::{Multiaddr, PeerId};
use serde::{Deserialize, Serialize};

pub const RELAY_CODE_PREFIX: &str = "reamesh://relay/";
pub const RELAY_AUTH_PROTOCOL: &str = "/reamesh/relay-auth/1";
pub const RELAY_INFO_PROTOCOL: &str = "/reamesh/relay-info/1";
/// Domain separation for the auth proof (see `auth_proof`).
const AUTH_DOMAIN: &[u8] = b"reamesh-relay-auth/1";
const RELAY_CODE_MAX_LEN: usize = 1024;
/// Most IPs a code (or an invite link's relay entry) names.
pub const RELAY_MAX_IPS: usize = 8;
/// Longest label a code carries, in characters; the rest is cut off.
pub const RELAY_LABEL_MAX: usize = 64;

/// A relay access key (`k` in a relay code).
pub type AccessKey = [u8; 32];

pub const REASON_BAD_KEY: &str = "bad_key";
pub const REASON_BANNED: &str = "banned";
pub const REASON_FULL: &str = "full";
/// The key was right but its date has passed (see `key add --expires`).
pub const REASON_EXPIRED: &str = "expired";

/// What this relay build can do, as one number: raised when a relay learns
/// something an app may come to rely on. Relays stay compatible with older apps
/// by only ever adding: nothing here or in the auth exchange is removed.
pub const RELAY_LEVEL: u32 = 1;
/// The level this app needs from a relay. Raise it (to at most `RELAY_LEVEL`)
/// only when the app stops working properly with less; below it the app says the
/// relay is outdated. A relay without the info protocol counts as level 0.
pub const RELAY_REQUIRED_LEVEL: u32 = 1;

/// How long a relay may sit on an `InfoRequest` whose answer hasn't changed.
pub const INFO_HOLD_SECS: u64 = 240;
/// The request timeout both sides need for such a held request.
pub const INFO_TIMEOUT_SECS: u64 = INFO_HOLD_SECS + 60;
/// Longest texts an `InfoResponse` carries, in characters.
pub const NOTICE_TEXT_MAX: usize = 200;
pub const RELAY_NAME_MAX: usize = 64;
pub const RELAY_CONTACT_MAX: usize = 120;
const VERSION_MAX: usize = 32;
/// From this share of a budget on, the answer counts as changed (and the app
/// warns).
pub const BUDGET_WARN_PERCENT: u8 = 90;

/// Where a relay listens (everything in a code but the key and label).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayAddr {
    pub peer: PeerId,
    pub ips: Vec<IpAddr>,
    /// DNS name to dial instead of (or besides) the IPs, "" for none.
    pub dns: String,
    /// 0 = the relay doesn't listen on that transport.
    pub tcp: u16,
    pub quic: u16,
}

/// A relay code: where the relay is, plus the access key and the operator's
/// label for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayCode {
    pub addr: RelayAddr,
    pub key: [u8; 32],
    pub label: String,
}

impl RelayAddr {
    /// Every address to dial the relay at (each IP and the DNS name, over
    /// TCP and QUIC), without the trailing `/p2p/<relay>`.
    pub fn dial_addrs(&self) -> Vec<Multiaddr> {
        let mut bases: Vec<Multiaddr> = self.ips.iter().map(|ip| Multiaddr::from(*ip)).collect();
        if !self.dns.is_empty() {
            bases.push(Multiaddr::empty().with(Protocol::Dns(self.dns.clone().into())));
        }
        let mut out = Vec::with_capacity(bases.len() * 2);
        for base in bases {
            if self.tcp != 0 {
                out.push(base.clone().with(Protocol::Tcp(self.tcp)));
            }
            if self.quic != 0 {
                out.push(base.with(Protocol::Udp(self.quic)).with(Protocol::QuicV1));
            }
        }
        out
    }

    /// `dial_addrs`, each ending in `/p2p/<relay>`.
    pub fn relay_addrs(&self) -> Vec<Multiaddr> {
        self.dial_addrs().into_iter().map(|a| a.with(Protocol::P2p(self.peer))).collect()
    }

    /// The circuit addresses that reach a peer reserved on this relay (the
    /// destination's `/p2p/<id>` is appended by whoever dials).
    pub fn circuit_addrs(&self) -> Vec<Multiaddr> {
        self.relay_addrs().into_iter().map(|a| a.with(Protocol::P2pCircuit)).collect()
    }

    /// What to call the relay when its operator gave no label: the DNS name,
    /// else the first IP.
    pub fn host(&self) -> String {
        if !self.dns.is_empty() {
            return self.dns.clone();
        }
        self.ips.first().map(|ip| ip.to_string()).unwrap_or_default()
    }

    /// Whether it names at least one place and one port.
    pub fn is_usable(&self) -> bool {
        !(self.ips.is_empty() && self.dns.is_empty()) && (self.tcp != 0 || self.quic != 0)
    }
}

impl RelayCode {
    /// The code in its canonical form (what `parse_relay_code` reads back).
    pub fn encode(&self) -> String {
        let a = &self.addr;
        let mut s = format!("{RELAY_CODE_PREFIX}{}?", a.peer);
        if !a.ips.is_empty() {
            let ips: Vec<String> = a.ips.iter().map(IpAddr::to_string).collect();
            let _ = write!(s, "a={}&", ips.join(","));
        }
        if !a.dns.is_empty() {
            let _ = write!(s, "d={}&", percent_encode(&a.dns));
        }
        if a.tcp != 0 {
            let _ = write!(s, "t={}&", a.tcp);
        }
        if a.quic != 0 {
            let _ = write!(s, "q={}&", a.quic);
        }
        s.push_str("k=");
        for b in self.key {
            let _ = write!(s, "{b:02x}");
        }
        let _ = write!(s, "&n={}", percent_encode(&self.label));
        s
    }
}

/// An IP a relay code may name: never unspecified, link-local, multicast or
/// broadcast, and loopback only with `allow_loopback` (tests run relays there).
pub fn dialable_relay_ip(ip: &IpAddr, allow_loopback: bool) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            (allow_loopback || !v4.is_loopback())
                && !v4.is_unspecified()
                && !v4.is_link_local()
                && !v4.is_multicast()
                && !v4.is_broadcast()
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return dialable_relay_ip(&IpAddr::V4(v4), allow_loopback);
            }
            (allow_loopback || !v6.is_loopback())
                && !v6.is_unspecified()
                && !v6.is_multicast()
                && (v6.segments()[0] & 0xffc0) != 0xfe80
        }
    }
}

fn parse_ips(v: &str, allow_loopback: bool) -> Vec<IpAddr> {
    v.split(',')
        .filter_map(|ip| ip.parse().ok())
        .filter(|ip| dialable_relay_ip(ip, allow_loopback))
        .take(RELAY_MAX_IPS)
        .collect()
}

/// A hostname a relay code may name: letters, digits, `-` and `.`, labels of
/// 1..=63 characters, at most 253 in all. Anything else (an IP literal belongs
/// in `a`, a port or path in nothing) is refused.
pub fn valid_dns_name(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 253
        && d.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        && d.parse::<IpAddr>().is_err()
}

/// Parse a relay code. `None` for anything malformed: a missing/odd key, no
/// place to dial, no port. Values are percent-decoded before they are read.
pub fn parse_relay_code(code: &str, allow_loopback: bool) -> Option<RelayCode> {
    let c = code.trim();
    if c.len() > RELAY_CODE_MAX_LEN
        || !c.get(..RELAY_CODE_PREFIX.len())?.eq_ignore_ascii_case(RELAY_CODE_PREFIX)
    {
        return None;
    }
    let rest = c.get(RELAY_CODE_PREFIX.len()..)?;
    let (peer, query) = rest.split_once('?').unwrap_or((rest, ""));
    let peer: PeerId = peer.trim_end_matches('/').parse().ok()?;
    let (mut ips, mut dns, mut tcp, mut quic) = (Vec::new(), String::new(), 0u16, 0u16);
    let (mut key, mut label) = (None, String::new());
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "a" => ips = parse_ips(&percent_decode(v)?, allow_loopback),
            "d" if !v.is_empty() => {
                let d = percent_decode(v)?;
                if !valid_dns_name(&d) {
                    return None;
                }
                dns = d.to_ascii_lowercase();
            }
            "t" => tcp = v.parse().ok().filter(|p| *p != 0)?,
            "q" => quic = v.parse().ok().filter(|p| *p != 0)?,
            "k" => key = Some(parse_key(v)?),
            // A garbled label is cosmetic: show none rather than refuse.
            "n" => label = percent_decode(v).unwrap_or_default().chars().take(RELAY_LABEL_MAX).collect(),
            _ => {}
        }
    }
    let addr = RelayAddr { peer, ips, dns, tcp, quic };
    if !addr.is_usable() {
        return None;
    }
    Some(RelayCode { addr, key: key?, label })
}

/// 64 lowercase hex characters → 32 bytes.
fn parse_key(v: &str) -> Option<[u8; 32]> {
    if v.len() != 64 || !v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&v[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Everything but the RFC 3986 unreserved characters, as `%XX`.
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

/// `None` on a malformed escape or invalid UTF-8; `+` is left alone.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The proof of the relay key, bound to both peer ids so it is useless to
/// anyone else and against any other relay.
pub fn auth_proof(key: &[u8; 32], relay: &PeerId, own: &PeerId) -> [u8; 32] {
    let mut msg = Vec::with_capacity(AUTH_DOMAIN.len() + 80);
    msg.extend_from_slice(AUTH_DOMAIN);
    msg.extend_from_slice(&relay.to_bytes());
    msg.extend_from_slice(&own.to_bytes());
    *blake3::keyed_hash(key, &msg).as_bytes()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthRequest {
    #[serde(with = "serde_bytes")]
    pub proof: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthResponse {
    pub ok: bool,
    /// `bad_key` | `banned` | `full` when refused.
    pub reason: Option<String>,
    /// The operator's name for the key that matched.
    pub key_label: Option<String>,
}

impl AuthResponse {
    pub fn accepted(label: &str) -> Self {
        Self { ok: true, reason: None, key_label: Some(label.to_owned()) }
    }

    pub fn refused(reason: &str) -> Self {
        Self { ok: false, reason: Some(reason.to_owned()), key_label: None }
    }
}

/// The label of the key `proof` was made with, if any. Every key is checked,
/// each in constant time (`blake3::Hash` equality), so neither timing nor an
/// early exit reveals which key came close.
pub fn verify_proof<'a>(
    keys: impl IntoIterator<Item = (&'a str, &'a [u8; 32])>,
    relay: &PeerId,
    client: &PeerId,
    proof: &[u8],
) -> Option<&'a str> {
    let given = blake3::Hash::from_bytes(<[u8; 32]>::try_from(proof).ok()?);
    let mut found = None;
    for (label, key) in keys {
        if blake3::Hash::from_bytes(auth_proof(key, relay, client)) == given && found.is_none() {
            found = Some(label);
        }
    }
    found
}

/// An announcement by the relay's operator. `from` and `until` are Unix
/// seconds, 0 for open-ended; each client shows them in its own time zone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notice {
    pub text: String,
    pub from: u64,
    pub until: u64,
}

/// A key holder asking how the relay stands.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InfoRequest {
    /// `auth_proof`, the same one `/reamesh/relay-auth/1` takes.
    #[serde(with = "serde_bytes")]
    pub proof: Vec<u8>,
    /// The `rev` of the last answer: the relay holds the request until its
    /// answer differs (at most `INFO_HOLD_SECS`). 0 answers at once.
    pub rev: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InfoResponse {
    pub ok: bool,
    /// `bad_key` | `banned` | `expired` when `ok` is false. An expired key
    /// still gets the operator's name and contact.
    pub reason: Option<String>,
    /// Identifies this answer's content (see `content_rev`).
    pub rev: u64,
    pub version: String,
    /// `RELAY_LEVEL` of that build.
    pub level: u32,
    /// `full` | `coordination` (only helps connections go direct).
    pub mode: String,
    /// Share of the relay's monthly budget used, 0 without a budget.
    pub budget_percent: u8,
    pub budget_exhausted: bool,
    /// The same for this key's own budget.
    pub key_budget_percent: u8,
    pub key_budget_exhausted: bool,
    /// When the budgets start over, Unix seconds.
    pub budget_resets: u64,
    pub hosts: u32,
    pub max_hosts: u32,
    pub notice: Option<Notice>,
    /// Set by the operator, both optional.
    pub name: String,
    pub contact: String,
    /// The first second this key no longer works, 0 for never.
    pub key_expires: u64,
    /// The relay is going away right now (stopping, or reconnecting everyone
    /// once to apply a change).
    pub stopping: bool,
    /// Where the relay says it can be reached.
    pub ips: Vec<IpAddr>,
    pub dns: String,
    pub tcp: u16,
    pub quic: u16,
}

impl InfoResponse {
    pub fn refused(reason: &str) -> Self {
        Self { reason: Some(reason.to_owned()), ..Self::default() }
    }

    /// What decides whether a held request gets answered: everything a client
    /// would act on, but not the numbers that move all the time (the host
    /// count, the budget below the warning mark). Never 0.
    pub fn content_rev(&self) -> u64 {
        let warn = |p: u8| p >= BUDGET_WARN_PERCENT;
        let mut h = blake3::Hasher::new();
        let text = format!(
            "{:?}",
            (
                (self.ok, &self.reason, &self.version, &self.mode),
                (warn(self.budget_percent), self.budget_exhausted),
                (warn(self.key_budget_percent), self.key_budget_exhausted, self.budget_resets),
                (&self.notice, &self.name, &self.contact, self.key_expires, self.stopping),
                (&self.ips, &self.dns, self.tcp, self.quic),
            )
        );
        h.update(text.as_bytes());
        let b = h.finalize();
        u64::from_le_bytes(b.as_bytes()[..8].try_into().unwrap()).max(1)
    }

    /// The same answer with everything bounded and single-line: this comes
    /// from a relay and ends up on screen.
    pub fn sanitized(mut self) -> Self {
        fn line(s: &str, max: usize) -> String {
            s.chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .take(max)
                .collect::<String>()
                .trim()
                .to_owned()
        }
        self.version = line(&self.version, VERSION_MAX);
        self.mode = if self.mode == "coordination" { self.mode } else { "full".to_owned() };
        self.budget_percent = self.budget_percent.min(100);
        self.key_budget_percent = self.key_budget_percent.min(100);
        self.name = line(&self.name, RELAY_NAME_MAX);
        self.contact = line(&self.contact, RELAY_CONTACT_MAX);
        self.notice = self.notice.take().and_then(|n| {
            let text = line(&n.text, NOTICE_TEXT_MAX);
            (!text.is_empty()).then_some(Notice { text, ..n })
        });
        self.ips.truncate(RELAY_MAX_IPS);
        self
    }

    /// The address a client should keep for this relay, when it differs from
    /// the one it has (`None` otherwise). The peer id stays pinned, so the
    /// relay can't send anyone elsewhere; what it can get wrong is its own
    /// address, so the one that just worked (`connected`) always stays, and a
    /// code whose port differs from what the relay believes it listens on
    /// (a mapped port, a hand-edited code) is left alone.
    pub fn updated_addr(
        &self,
        current: &RelayAddr,
        connected: Option<&Multiaddr>,
        allow_loopback: bool,
    ) -> Option<RelayAddr> {
        let dns = if self.dns.is_empty() {
            current.dns.clone()
        } else if valid_dns_name(&self.dns) {
            self.dns.to_ascii_lowercase()
        } else {
            return None;
        };
        let mut ips: Vec<IpAddr> = Vec::new();
        if let Some(addr) = connected {
            let (mut ip, mut tcp, mut udp) = (None, None, None);
            for p in addr.iter() {
                match p {
                    Protocol::Ip4(a) => ip = Some(IpAddr::V4(a)),
                    Protocol::Ip6(a) => ip = Some(IpAddr::V6(a)),
                    Protocol::Tcp(port) => tcp = Some(port),
                    Protocol::Udp(port) => udp = Some(port),
                    _ => {}
                }
            }
            if tcp.is_some_and(|p| p != self.tcp) || udp.is_some_and(|p| p != self.quic) {
                return None;
            }
            // Reached through the name: that keeps working as long as the name
            // stays, and its resolved address is no fact about the relay.
            let by_name = !current.dns.is_empty() && current.dns == dns;
            if let Some(ip) = ip.filter(|ip| !by_name && dialable_relay_ip(ip, allow_loopback)) {
                ips.push(ip);
            }
        }
        for ip in self.ips.iter().filter(|ip| dialable_relay_ip(ip, allow_loopback)) {
            if ips.len() < RELAY_MAX_IPS && !ips.contains(ip) {
                ips.push(*ip);
            }
        }
        let next = RelayAddr {
            peer: current.peer,
            ips,
            dns,
            tcp: self.tcp,
            quic: self.quic,
        };
        let same_ips = next.ips.len() == current.ips.len()
            && next.ips.iter().all(|ip| current.ips.contains(ip));
        let same = same_ips && next.dns == current.dns && next.tcp == current.tcp && next.quic == current.quic;
        (!same && next.is_usable()).then_some(next)
    }
}

/// A refusal's reason as one of the stable codes; anything else (or none)
/// becomes `refused`.
pub fn refusal_reason(resp: &AuthResponse) -> &'static str {
    match resp.reason.as_deref() {
        Some(REASON_BAD_KEY) => REASON_BAD_KEY,
        Some(REASON_BANNED) => REASON_BANNED,
        Some(REASON_FULL) => REASON_FULL,
        Some(REASON_EXPIRED) => REASON_EXPIRED,
        _ => "refused",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: &str = "12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN";
    const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn parse(code: &str) -> Option<RelayCode> {
        parse_relay_code(code, false)
    }

    fn addr(ips: &[&str], dns: &str) -> RelayAddr {
        RelayAddr {
            peer: PEER.parse().unwrap(),
            ips: ips.iter().map(|ip| ip.parse().unwrap()).collect(),
            dns: dns.into(),
            tcp: 4001,
            quic: 4001,
        }
    }

    fn advertised(ips: &[&str], dns: &str) -> InfoResponse {
        InfoResponse {
            ok: true,
            ips: ips.iter().map(|ip| ip.parse().unwrap()).collect(),
            dns: dns.into(),
            tcp: 4001,
            quic: 4001,
            ..InfoResponse::default()
        }
    }

    #[test]
    fn a_new_relay_address_replaces_the_old_one_but_keeps_the_working_one() {
        let cur = addr(&["203.0.113.5"], "");
        let tcp: Multiaddr = "/ip4/203.0.113.5/tcp/4001".parse().unwrap();
        // Same address: nothing to do.
        assert_eq!(advertised(&["203.0.113.5"], "").updated_addr(&cur, Some(&tcp), false), None);
        // A second address joins the one that works.
        let next = advertised(&["203.0.113.9"], "").updated_addr(&cur, Some(&tcp), false).unwrap();
        assert_eq!(next.ips, addr(&["203.0.113.5", "203.0.113.9"], "").ips);
        // Without a connection the relay's word stands.
        let next = advertised(&["203.0.113.9"], "").updated_addr(&cur, None, false).unwrap();
        assert_eq!(next.ips, addr(&["203.0.113.9"], "").ips);
        // A relay that knows no address of its own changes nothing.
        assert_eq!(advertised(&[], "").updated_addr(&cur, None, false), None);
    }

    #[test]
    fn a_mapped_port_and_bad_addresses_leave_the_code_alone() {
        let cur = addr(&["203.0.113.5"], "");
        let mapped: Multiaddr = "/ip4/203.0.113.5/tcp/443".parse().unwrap();
        assert_eq!(advertised(&["203.0.113.9"], "").updated_addr(&cur, Some(&mapped), false), None);
        let quic: Multiaddr = "/ip4/203.0.113.5/udp/443/quic-v1".parse().unwrap();
        assert_eq!(advertised(&["203.0.113.9"], "").updated_addr(&cur, Some(&quic), false), None);
        // Undialable addresses never get in, and a bad name stops the update.
        assert_eq!(advertised(&["127.0.0.1"], "").updated_addr(&cur, None, false), None);
        assert_eq!(advertised(&["203.0.113.9"], "no such/name").updated_addr(&cur, None, false), None);
    }

    #[test]
    fn a_relay_reached_by_name_keeps_the_name() {
        let cur = addr(&[], "relay.example.org");
        let tcp: Multiaddr = "/ip4/203.0.113.5/tcp/4001".parse().unwrap();
        assert_eq!(advertised(&[], "relay.example.org").updated_addr(&cur, Some(&tcp), false), None);
        // The operator didn't name it: the code's name stays.
        assert_eq!(advertised(&[], "").updated_addr(&cur, Some(&tcp), false), None);
    }

    #[test]
    fn content_rev_ignores_what_moves_all_the_time() {
        let a = InfoResponse { ok: true, hosts: 1, budget_percent: 10, ..InfoResponse::default() };
        let b = InfoResponse { hosts: 7, budget_percent: 11, ..a.clone() };
        assert_eq!(a.content_rev(), b.content_rev());
        let warn = InfoResponse { budget_percent: BUDGET_WARN_PERCENT, ..a.clone() };
        assert_ne!(a.content_rev(), warn.content_rev());
        let notice = InfoResponse {
            notice: Some(Notice { text: "x".into(), from: 1, until: 2 }),
            ..a.clone()
        };
        assert_ne!(a.content_rev(), notice.content_rev());
    }

    #[test]
    fn sanitized_bounds_what_a_relay_sends() {
        let r = InfoResponse {
            mode: "<b>".into(),
            budget_percent: 250,
            name: format!(" a\nb{} ", "x".repeat(RELAY_NAME_MAX)),
            notice: Some(Notice { text: "   ".into(), from: 0, until: 0 }),
            ..InfoResponse::default()
        }
        .sanitized();
        assert_eq!(r.mode, "full");
        assert_eq!(r.budget_percent, 100);
        assert!(r.name.starts_with("a b"));
        assert!(r.name.chars().count() <= RELAY_NAME_MAX);
        assert_eq!(r.notice, None);
    }

    #[test]
    fn relay_code_round_trips_every_field() {
        let code = format!(
            "reamesh://relay/{PEER}?a=203.0.113.5,2001:db8::1&d=relay.example.org&t=4001&q=4002&k={KEY}&n=Studio%20Relay"
        );
        let c = parse(&code).expect("parses");
        assert_eq!(c.addr.peer.to_string(), PEER);
        assert_eq!(c.addr.ips.len(), 2);
        assert_eq!(c.addr.dns, "relay.example.org");
        assert_eq!((c.addr.tcp, c.addr.quic), (4001, 4002));
        assert_eq!(c.key[0], 0x00);
        assert_eq!(c.key[31], 0xff);
        assert_eq!(c.label, "Studio Relay");
        assert_eq!(c.encode(), code);
        assert_eq!(parse(&c.encode()), Some(c.clone()));
        // 3 places × 2 transports.
        assert_eq!(c.addr.dial_addrs().len(), 6);
        assert!(c.addr.dial_addrs().iter().any(|a| a.to_string() == "/dns/relay.example.org/tcp/4001"));
        assert!(c.addr.relay_addrs()[0].to_string().ends_with(&format!("/p2p/{PEER}")));
        assert!(c.addr.circuit_addrs()[0].to_string().ends_with(&format!("/p2p/{PEER}/p2p-circuit")));
        assert_eq!(c.addr.host(), "relay.example.org");
    }

    #[test]
    fn relay_code_reads_percent_encoded_values_and_odd_keys() {
        // What the relay server writes: every value percent-encoded.
        let code = format!("reamesh://relay/{PEER}?d=relay%2Eexample.org&t=4001&q=4001&k={KEY}&n=");
        let c = parse(&code).unwrap();
        assert_eq!(c.addr.dns, "relay.example.org");
        assert_eq!(c.label, "");
        let mut other = c.clone();
        other.key = [0xab; 32];
        other.label = "Ü & Co".into();
        assert_eq!(parse(&other.encode()), Some(other));
    }

    #[test]
    fn relay_code_refuses_malformed_input() {
        let ok = format!("reamesh://relay/{PEER}?a=203.0.113.5&t=4001&k={KEY}");
        assert!(parse(&ok).is_some());
        assert!(parse(&ok.to_uppercase()).is_none(), "peer ids are case-sensitive");
        assert!(parse(&ok.replacen("reamesh://relay/", "REAMESH://Relay/", 1)).is_some());
        // No key, short key, uppercase hex, no port, no place, bad DNS name.
        assert!(parse(&format!("reamesh://relay/{PEER}?a=203.0.113.5&t=4001")).is_none());
        assert!(parse(&format!("reamesh://relay/{PEER}?a=203.0.113.5&t=4001&k=abcd")).is_none());
        assert!(parse(&ok.replace(KEY, &KEY.to_uppercase())).is_none());
        assert!(parse(&format!("reamesh://relay/{PEER}?a=203.0.113.5&k={KEY}")).is_none());
        assert!(parse(&format!("reamesh://relay/{PEER}?t=4001&k={KEY}")).is_none());
        assert!(parse(&format!("reamesh://relay/{PEER}?d=bad_name&t=4001&k={KEY}")).is_none());
        assert!(parse(&format!("reamesh://relay/{PEER}?d=1.2.3.4&t=4001&k={KEY}")).is_none());
        assert!(parse(&format!("reamesh://relay/notapeer?a=203.0.113.5&t=4001&k={KEY}")).is_none());
        // Loopback is not a relay anyone else can reach (unless allowed).
        let lo = format!("reamesh://relay/{PEER}?a=127.0.0.1&t=4001&k={KEY}");
        assert!(parse(&lo).is_none());
        assert!(parse_relay_code(&lo, true).is_some());
        assert!(parse(&format!("reamesh://join/{PEER}?a=203.0.113.5&t=4001&k={KEY}")).is_none());
    }

    #[test]
    fn auth_proof_binds_key_and_both_peers() {
        let key = parse_key(KEY).unwrap();
        let (relay, a, b) = (PeerId::random(), PeerId::random(), PeerId::random());
        let p = auth_proof(&key, &relay, &a);
        assert_eq!(p, auth_proof(&key, &relay, &a));
        assert_ne!(p, auth_proof(&key, &relay, &b));
        assert_ne!(p, auth_proof(&key, &b, &a));
        let mut other = key;
        other[0] ^= 1;
        assert_ne!(p, auth_proof(&other, &relay, &a));
        // The same proof, written out.
        let mut msg = b"reamesh-relay-auth/1".to_vec();
        msg.extend_from_slice(&relay.to_bytes());
        msg.extend_from_slice(&a.to_bytes());
        assert_eq!(p, *blake3::keyed_hash(&key, &msg).as_bytes());
    }

    #[test]
    fn refusal_reasons_are_the_stable_codes() {
        let r = |reason: Option<&str>| AuthResponse { ok: false, reason: reason.map(Into::into), key_label: None };
        assert_eq!(refusal_reason(&r(Some("bad_key"))), "bad_key");
        assert_eq!(refusal_reason(&r(Some("banned"))), "banned");
        assert_eq!(refusal_reason(&r(Some("full"))), "full");
        assert_eq!(refusal_reason(&r(Some("weird"))), "refused");
        assert_eq!(refusal_reason(&r(None)), "refused");
    }

    #[test]
    fn relay_code_expands_every_host_in_order() {
        let peer = PeerId::random();
        let c = RelayCode {
            addr: RelayAddr {
                peer,
                ips: vec!["203.0.113.7".parse().unwrap(), "2001:db8::1".parse().unwrap()],
                dns: "relay.example.org".into(),
                tcp: 4001,
                quic: 443,
            },
            key: [0xab; 32],
            label: "Bens Band & Co/ü".into(),
        };
        let text = c.encode();
        assert!(text.contains("a=203.0.113.7,2001:db8::1&"));
        assert!(text.contains("n=Bens%20Band%20%26%20Co%2F%C3%BC"));
        assert_eq!(parse(&text), Some(c.clone()));
        let addrs: Vec<String> = c.addr.relay_addrs().iter().map(|a| a.to_string()).collect();
        assert_eq!(
            addrs,
            vec![
                format!("/ip4/203.0.113.7/tcp/4001/p2p/{peer}"),
                format!("/ip4/203.0.113.7/udp/443/quic-v1/p2p/{peer}"),
                format!("/ip6/2001:db8::1/tcp/4001/p2p/{peer}"),
                format!("/ip6/2001:db8::1/udp/443/quic-v1/p2p/{peer}"),
                format!("/dns/relay.example.org/tcp/4001/p2p/{peer}"),
                format!("/dns/relay.example.org/udp/443/quic-v1/p2p/{peer}"),
            ]
        );
        // A plus is a plus, as in the app's decodeURIComponent.
        let plus = text.replace("n=Bens%20Band%20%26%20Co%2F%C3%BC", "n=a+b");
        assert_eq!(parse(&plus).unwrap().label, "a+b");
    }

    #[test]
    fn verify_proof_finds_the_key_and_refuses_everything_else() {
        let (relay, client, other) = (PeerId::random(), PeerId::random(), PeerId::random());
        let keys = [("alice".to_owned(), [1u8; 32]), ("bob".to_owned(), [2u8; 32])];
        let iter = || keys.iter().map(|(l, k)| (l.as_str(), k));
        let p = auth_proof(&[2u8; 32], &relay, &client);
        assert_eq!(verify_proof(iter(), &relay, &client, &p), Some("bob"));
        assert_eq!(verify_proof(iter(), &relay, &client, &auth_proof(&[9u8; 32], &relay, &client)), None);
        // Made for another client, or for another relay: a replay fails.
        assert_eq!(verify_proof(iter(), &relay, &client, &auth_proof(&[1u8; 32], &relay, &other)), None);
        assert_eq!(verify_proof(iter(), &relay, &client, &auth_proof(&[1u8; 32], &other, &client)), None);
        assert_eq!(verify_proof(iter(), &relay, &client, &[0u8; 31]), None);
    }
}
