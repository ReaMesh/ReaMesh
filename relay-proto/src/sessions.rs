//! The relay's optional session list: two request-response protocols (CBOR,
//! like the auth exchange) and the signed entry they carry.
//!
//! `/reamesh/relay-sessions/1`: a key holder asks for the list. Every request
//! carries the auth proof, so looking at the list takes no login and no host
//! place. A request whose `rev` equals the relay's current one is held until
//! the list changes (at most `SESSIONS_HOLD_SECS`).
//!
//! `/reamesh/relay-publish/1`: a host with a reservation stores its entry, or
//! withdraws it with `envelope: null`.
//!
//! An entry is a libp2p `SignedEnvelope` signed with the host's identity and
//! bound to one relay. The relay checks only who signed it and passes the rest
//! on untouched, so entries can grow without a relay update. Clients trust
//! what `open_entry` returns, never what the relay says about an entry.

use libp2p::PeerId;
use libp2p::core::SignedEnvelope;
use libp2p::identity::Keypair;
use serde::{Deserialize, Serialize};

pub const RELAY_SESSIONS_PROTOCOL: &str = "/reamesh/relay-sessions/1";
pub const RELAY_PUBLISH_PROTOCOL: &str = "/reamesh/relay-publish/1";

/// The envelope's domain separation and payload type.
const ENTRY_DOMAIN: &str = "reamesh-relay-session";
const ENTRY_PAYLOAD_TYPE: &[u8] = b"/reamesh/relay-session/1";
const ENTRY_VERSION: u32 = 1;
/// Largest entry a relay stores or a client reads, envelope included.
pub const ENTRY_MAX_BYTES: usize = 4096;

/// How long a request with the current `rev` is held before it is answered
/// unchanged.
pub const SESSIONS_HOLD_SECS: u64 = 240;
/// Both sides' request timeout: longer than a hold, or the stream is cut.
pub const SESSIONS_TIMEOUT_SECS: u64 = SESSIONS_HOLD_SECS + 60;

/// This relay keeps no list.
pub const REASON_DISABLED: &str = "disabled";
/// A publish without a reservation.
pub const REASON_NOT_RESERVED: &str = "not_reserved";
pub const REASON_TOO_LARGE: &str = "too_large";
/// A publish not signed by the connection that sent it.
pub const REASON_BAD_ENTRY: &str = "bad_entry";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionsRequest {
    #[serde(with = "serde_bytes")]
    pub proof: Vec<u8>,
    /// The `rev` of the last answer; 0 is answered at once.
    #[serde(default)]
    pub rev: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedEntry {
    #[serde(with = "serde_bytes")]
    pub envelope: Vec<u8>,
    /// The operator's name for the key the host used. It comes from the relay,
    /// everything inside the envelope from the host.
    #[serde(default)]
    pub key_label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionsResponse {
    pub ok: bool,
    #[serde(default)]
    pub reason: Option<String>,
    /// Whether this relay keeps a list at all.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub rev: u64,
    #[serde(default)]
    pub entries: Vec<ListedEntry>,
    /// The month's budget is used up: circuits only carry enough to find a
    /// direct path.
    #[serde(default)]
    pub budget_used: bool,
}

impl SessionsResponse {
    pub fn refused(reason: &str) -> Self {
        Self { reason: Some(reason.to_owned()), ..Self::default() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishRequest {
    #[serde(with = "serde_bytes")]
    pub proof: Vec<u8>,
    /// The signed entry; `None` withdraws it.
    #[serde(default, with = "serde_bytes")]
    pub envelope: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishResponse {
    pub ok: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

impl PublishResponse {
    pub fn accepted() -> Self {
        Self { ok: true, reason: None }
    }

    pub fn refused(reason: &str) -> Self {
        Self { ok: false, reason: Some(reason.to_owned()) }
    }
}

/// What a host says about its session, as JSON inside the envelope. Fields a
/// later version adds stay in `extra`, so re-encoding an entry keeps them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionEntry {
    pub v: u32,
    /// The relay this entry is for (`open_entry` refuses it on any other).
    pub relay: String,
    pub session_id: String,
    pub session_name: String,
    pub owner: String,
    #[serde(default)]
    pub owner_name: String,
    #[serde(default)]
    pub owner_color: String,
    #[serde(default)]
    pub auth: bool,
    #[serde(default)]
    pub salt: String,
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub proto: u32,
    #[serde(default)]
    pub ver: String,
    /// An invite link that names this relay and no direct address.
    pub ticket: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl SessionEntry {
    pub const VERSION: u32 = ENTRY_VERSION;
}

/// Why `open_entry` refused an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryError {
    TooLarge,
    /// Not an envelope, not JSON, or a version this build can't read.
    Malformed,
    BadSignature,
    /// Signed by someone else than the `owner` it names.
    WrongOwner,
    /// Made for another relay.
    WrongRelay,
    /// The ticket names another host, another session, a direct address or
    /// another relay.
    BadTicket,
}

/// Signs an entry for publishing. `None` when it doesn't fit `ENTRY_MAX_BYTES`
/// (a very long session or host name).
pub fn seal_entry(key: &Keypair, entry: &SessionEntry) -> Option<Vec<u8>> {
    let payload = serde_json::to_vec(entry).ok()?;
    let envelope =
        SignedEnvelope::new(key, ENTRY_DOMAIN.to_owned(), ENTRY_PAYLOAD_TYPE.to_vec(), payload).ok()?;
    Some(envelope.into_protobuf_encoding()).filter(|b| b.len() <= ENTRY_MAX_BYTES)
}

/// Who signed an envelope, the signature checked. All a relay reads of it.
pub fn envelope_signer(bytes: &[u8]) -> Option<PeerId> {
    let envelope = SignedEnvelope::from_protobuf_encoding(bytes).ok()?;
    let (_, key) = envelope
        .payload_and_signing_key(ENTRY_DOMAIN.to_owned(), ENTRY_PAYLOAD_TYPE)
        .ok()?;
    Some(key.to_peer_id())
}

/// Checks an entry as a client must before using anything in it: the signature
/// is the owner's, it was made for this relay, and its ticket can only lead to
/// the owner through this relay.
pub fn open_entry(bytes: &[u8], relay: &PeerId) -> Result<SessionEntry, EntryError> {
    if bytes.len() > ENTRY_MAX_BYTES {
        return Err(EntryError::TooLarge);
    }
    let envelope = SignedEnvelope::from_protobuf_encoding(bytes).map_err(|_| EntryError::Malformed)?;
    let (payload, key) = envelope
        .payload_and_signing_key(ENTRY_DOMAIN.to_owned(), ENTRY_PAYLOAD_TYPE)
        .map_err(|_| EntryError::BadSignature)?;
    let entry: SessionEntry = serde_json::from_slice(payload).map_err(|_| EntryError::Malformed)?;
    if entry.v != ENTRY_VERSION {
        return Err(EntryError::Malformed);
    }
    let signer = key.to_peer_id();
    if entry.owner.parse::<PeerId>().ok() != Some(signer) {
        return Err(EntryError::WrongOwner);
    }
    if entry.relay.parse::<PeerId>().ok().as_ref() != Some(relay) {
        return Err(EntryError::WrongRelay);
    }
    if !ticket_fits(&entry, relay) {
        return Err(EntryError::BadTicket);
    }
    Ok(entry)
}

/// The ticket is for this session and host, and names this relay and nothing
/// else to dial. The full parser lives with the core; this only has to tell
/// where a client would be sent.
fn ticket_fits(entry: &SessionEntry, relay: &PeerId) -> bool {
    const PREFIX: &str = "reamesh://join/";
    let t = entry.ticket.as_str();
    let Some(rest) = t.get(PREFIX.len()..).filter(|_| t[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)) else {
        return false;
    };
    let Some((sid, query)) = rest.split_once('?') else { return false };
    if sid != entry.session_id {
        return false;
    }
    let (mut host, mut relays) = (None, 0);
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "p" => host = Some(v),
            "a" | "t" | "q" if !v.is_empty() => return false,
            "r" => {
                if v.split('~').next().and_then(|p| p.parse::<PeerId>().ok()).as_ref() != Some(relay) {
                    return false;
                }
                relays += 1;
            }
            _ => {}
        }
    }
    relays == 1 && host == Some(entry.owner.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &Keypair, relay: &PeerId) -> SessionEntry {
        let owner = key.public().to_peer_id();
        SessionEntry {
            v: SessionEntry::VERSION,
            relay: relay.to_string(),
            session_id: "abc123".into(),
            session_name: "Film Cue".into(),
            owner: owner.to_string(),
            owner_name: "Sam".into(),
            owner_color: "#f0a35e".into(),
            auth: false,
            salt: String::new(),
            files: 42,
            bytes: 1_331_439_862,
            proto: 7,
            ver: "1.2.0".into(),
            ticket: format!(
                "reamesh://join/abc123?n=Film%20Cue&p={owner}&a=&x=00112233445566778899aabbccddeeff&r={relay}~203.0.113.5~4001~4001"
            ),
            extra: serde_json::Map::new(),
        }
    }

    fn setup() -> (Keypair, PeerId) {
        (Keypair::generate_ed25519(), PeerId::random())
    }

    #[test]
    fn a_sealed_entry_opens_on_its_relay() {
        let (key, relay) = setup();
        let e = entry(&key, &relay);
        let bytes = seal_entry(&key, &e).unwrap();
        assert_eq!(open_entry(&bytes, &relay), Ok(e));
        assert_eq!(envelope_signer(&bytes), Some(key.public().to_peer_id()));
    }

    #[test]
    fn a_changed_signature_is_refused() {
        let (key, relay) = setup();
        let mut bytes = seal_entry(&key, &entry(&key, &relay)).unwrap();
        // The signature is the end of the encoded envelope.
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert_eq!(open_entry(&bytes, &relay), Err(EntryError::BadSignature));
        assert_eq!(envelope_signer(&bytes), None);
    }

    #[test]
    fn an_entry_signed_by_someone_else_is_refused() {
        let (key, relay) = setup();
        // A relay or anyone else can't speak for a known host.
        let e = entry(&key, &relay);
        let forged = seal_entry(&Keypair::generate_ed25519(), &e).unwrap();
        assert_eq!(open_entry(&forged, &relay), Err(EntryError::WrongOwner));
    }

    #[test]
    fn an_entry_for_another_relay_is_refused() {
        let (key, relay) = setup();
        let bytes = seal_entry(&key, &entry(&key, &relay)).unwrap();
        assert_eq!(open_entry(&bytes, &PeerId::random()), Err(EntryError::WrongRelay));
    }

    #[test]
    fn a_ticket_for_another_host_is_refused() {
        let (key, relay) = setup();
        let mut e = entry(&key, &relay);
        e.ticket = e.ticket.replace(&e.owner, &PeerId::random().to_string());
        let bytes = seal_entry(&key, &e).unwrap();
        assert_eq!(open_entry(&bytes, &relay), Err(EntryError::BadTicket));
    }

    #[test]
    fn a_ticket_with_direct_addresses_or_other_relays_is_refused() {
        let (key, relay) = setup();
        let base = entry(&key, &relay);
        let other = PeerId::random();
        for ticket in [
            base.ticket.replace("&a=&", "&a=198.51.100.9&t=4001&"),
            base.ticket.replace("&a=&", "&a=&q=4001&"),
            format!("{}&r={other}~198.51.100.9~4001~4001", base.ticket),
            base.ticket.replace(&format!("r={relay}"), &format!("r={other}")),
            base.ticket.replace(&format!("&r={relay}~203.0.113.5~4001~4001"), ""),
            base.ticket.replace("join/abc123", "join/other1"),
            base.ticket.replace("reamesh://join/", "reamesh://relay/"),
            "short".to_owned(),
        ] {
            let e = SessionEntry { ticket: ticket.clone(), ..base.clone() };
            let bytes = seal_entry(&key, &e).unwrap();
            assert_eq!(open_entry(&bytes, &relay), Err(EntryError::BadTicket), "{ticket}");
        }
    }

    #[test]
    fn unknown_fields_survive() {
        let (key, relay) = setup();
        let mut e = entry(&key, &relay);
        e.extra.insert("later".into(), serde_json::json!({ "a": [1, 2] }));
        let bytes = seal_entry(&key, &e).unwrap();
        let back = open_entry(&bytes, &relay).unwrap();
        assert_eq!(back.extra["later"]["a"][1], 2);
        assert_eq!(back, e);
        let again = serde_json::to_value(&back).unwrap();
        assert_eq!(again["later"]["a"][0], 1);
    }

    #[test]
    fn another_version_and_garbage_are_refused() {
        let (key, relay) = setup();
        let e = SessionEntry { v: 2, ..entry(&key, &relay) };
        let bytes = seal_entry(&key, &e).unwrap();
        assert_eq!(open_entry(&bytes, &relay), Err(EntryError::Malformed));
        assert_eq!(open_entry(b"nonsense", &relay), Err(EntryError::Malformed));
        assert_eq!(open_entry(&vec![0u8; ENTRY_MAX_BYTES + 1], &relay), Err(EntryError::TooLarge));
    }

    #[test]
    fn an_entry_over_the_limit_is_not_sealed() {
        let (key, relay) = setup();
        let e = SessionEntry { session_name: "x".repeat(ENTRY_MAX_BYTES), ..entry(&key, &relay) };
        assert_eq!(seal_entry(&key, &e), None);
    }

    #[test]
    fn missing_response_fields_take_their_defaults() {
        // What an older or newer relay may leave out, as JSON to stay free of
        // a CBOR dependency here; serde treats both formats alike.
        let r: SessionsResponse = serde_json::from_str(r#"{"ok":true}"#).unwrap();
        assert_eq!(r, SessionsResponse { ok: true, ..SessionsResponse::default() });
        let r: PublishResponse = serde_json::from_str(r#"{"ok":false,"more":1}"#).unwrap();
        assert_eq!(r, PublishResponse { ok: false, reason: None });
    }
}
