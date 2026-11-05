//! The optional session list: what hosts published, in memory only. Nothing
//! in here reaches `status.json` or the log beyond counts, and the entries stay
//! opaque: the relay reads who signed one, never what it says.
//!
//! Lobby viewers aren't tracked anywhere else: their proof comes with every
//! request, so they take no host place. A request with the current `rev` waits
//! here until the list changes. Changes are answered on the relay's tick, so
//! however fast entries change, viewers get at most one answer per tick.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use libp2p::PeerId;
use libp2p::swarm::ConnectionId;
use reamesh_relay_proto::sessions::{
    self, ENTRY_MAX_BYTES, ListedEntry, PublishResponse, REASON_BAD_ENTRY, REASON_DISABLED,
    REASON_NOT_RESERVED, REASON_TOO_LARGE, SESSIONS_HOLD_SECS, SessionsResponse,
};
use reamesh_relay_proto::{REASON_BAD_KEY, REASON_BANNED, REASON_EXPIRED};

/// Requests held at once; past this they are answered right away and asked
/// again by the client, which costs little and holds nothing.
const MAX_HELD: usize = 1024;

/// What the caller found out about a request's proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access<'a> {
    Key(&'a str),
    BadKey,
    Banned,
    /// A key past its last day.
    Expired,
}

impl Access<'_> {
    fn refusal(self) -> Option<&'static str> {
        match self {
            Access::Key(_) => None,
            Access::BadKey => Some(REASON_BAD_KEY),
            Access::Banned => Some(REASON_BANNED),
            Access::Expired => Some(REASON_EXPIRED),
        }
    }
}

struct Listed {
    envelope: Vec<u8>,
    key_label: String,
    proof: Vec<u8>,
}

struct Held<C> {
    channel: C,
    rev: u64,
    proof: Vec<u8>,
    since: Instant,
}

/// `C` is how a held request gets answered (a response channel on the swarm).
pub struct SessionList<C> {
    enabled: bool,
    budget_used: bool,
    rev: u64,
    entries: BTreeMap<PeerId, Listed>,
    /// Per connection, not per peer: a host's hosting run and its own lobby
    /// or Settings can each wait on a connection of their own.
    held: HashMap<(PeerId, ConnectionId), Held<C>>,
}

impl<C> SessionList<C> {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            budget_used: false,
            rev: 1,
            entries: BTreeMap::new(),
            held: HashMap::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Viewers waiting for a change right now.
    pub fn watching(&self) -> usize {
        self.held.len()
    }

    pub fn is_listed(&self, peer: &PeerId) -> bool {
        self.entries.contains_key(peer)
    }

    fn answer(&self) -> SessionsResponse {
        SessionsResponse {
            ok: true,
            reason: None,
            enabled: self.enabled,
            rev: self.rev,
            entries: self
                .entries
                .values()
                .map(|l| ListedEntry {
                    envelope: l.envelope.clone(),
                    key_label: l.key_label.clone(),
                })
                .collect(),
            budget_used: self.budget_used,
        }
    }

    /// A request for the list. Answers to send now: this one, unless it is
    /// held, and whatever the same connection still had waiting.
    #[allow(clippy::too_many_arguments)]
    pub fn query(
        &mut self,
        peer: PeerId,
        conn: ConnectionId,
        access: Access<'_>,
        proof: &[u8],
        rev: u64,
        channel: C,
        now: Instant,
    ) -> Vec<(C, SessionsResponse)> {
        let mut out = Vec::new();
        if let Some(old) = self.held.remove(&(peer, conn)) {
            out.push((old.channel, self.answer()));
        }
        if let Some(reason) = access.refusal() {
            out.push((channel, SessionsResponse::refused(reason)));
        } else if rev == self.rev && self.held.len() < MAX_HELD {
            self.held.insert(
                (peer, conn),
                Held {
                    channel,
                    rev,
                    proof: proof.to_vec(),
                    since: now,
                },
            );
        } else {
            out.push((channel, self.answer()));
        }
        out
    }

    /// A publish from `peer`. `envelope: None` withdraws, which takes no
    /// reservation: a host that just lost its own should still be heard.
    pub fn publish(
        &mut self,
        peer: PeerId,
        access: Access<'_>,
        proof: &[u8],
        envelope: Option<Vec<u8>>,
        reserved: bool,
    ) -> PublishResponse {
        let Access::Key(label) = access else {
            return PublishResponse::refused(access.refusal().unwrap_or(REASON_BAD_KEY));
        };
        if !self.enabled {
            return PublishResponse::refused(REASON_DISABLED);
        }
        let Some(envelope) = envelope else {
            self.withdraw(&peer);
            return PublishResponse::accepted();
        };
        if !reserved {
            return PublishResponse::refused(REASON_NOT_RESERVED);
        }
        if envelope.len() > ENTRY_MAX_BYTES {
            return PublishResponse::refused(REASON_TOO_LARGE);
        }
        if sessions::envelope_signer(&envelope) != Some(peer) {
            return PublishResponse::refused(REASON_BAD_ENTRY);
        }
        let same = self.entries.get(&peer).is_some_and(|l| {
            l.envelope == envelope && l.key_label == label
        });
        if !same {
            self.entries.insert(
                peer,
                Listed {
                    envelope,
                    key_label: label.to_owned(),
                    proof: proof.to_vec(),
                },
            );
            self.rev += 1;
        }
        PublishResponse::accepted()
    }

    /// The host's reservation is gone, or it took the session off the list.
    pub fn withdraw(&mut self, peer: &PeerId) {
        if self.entries.remove(peer).is_some() {
            self.rev += 1;
        }
    }

    /// `list on` / `list off`. Off forgets every entry on the spot.
    pub fn set_enabled(&mut self, on: bool) {
        if self.enabled != on {
            self.enabled = on;
            self.entries.clear();
            self.rev += 1;
        }
    }

    pub fn set_budget_used(&mut self, used: bool) {
        if self.budget_used != used {
            self.budget_used = used;
            self.rev += 1;
        }
    }

    /// The keys or bans changed: `check` says what a stored proof is worth
    /// now. Entries it no longer backs go at once, and so do held requests,
    /// with the refusal as their answer.
    pub fn recheck<'a>(
        &mut self,
        mut check: impl FnMut(&PeerId, &[u8]) -> Access<'a>,
    ) -> Vec<(C, SessionsResponse)> {
        let before = self.entries.len();
        self.entries
            .retain(|peer, l| matches!(check(peer, &l.proof), Access::Key(_)));
        if self.entries.len() != before {
            self.rev += 1;
        }
        let mut out = Vec::new();
        for (who, held) in std::mem::take(&mut self.held) {
            match check(&who.0, &held.proof).refusal() {
                Some(reason) => out.push((held.channel, SessionsResponse::refused(reason))),
                None => {
                    self.held.insert(who, held);
                }
            }
        }
        out
    }

    /// Requests to answer on this tick: the list changed since they asked, or
    /// they waited long enough. `open` drops the ones nobody listens to anymore.
    pub fn due(
        &mut self,
        now: Instant,
        open: impl Fn(&C) -> bool,
    ) -> Vec<(C, SessionsResponse)> {
        let hold = Duration::from_secs(SESSIONS_HOLD_SECS);
        let mut out = Vec::new();
        for (who, held) in std::mem::take(&mut self.held) {
            if !open(&held.channel) {
                continue;
            }
            if held.rev != self.rev || now.duration_since(held.since) >= hold {
                out.push((held.channel, self.answer()));
            } else {
                self.held.insert(who, held);
            }
        }
        out
    }

    /// Every connection is about to drop (the swarm is being rebuilt). `rev`
    /// goes on counting, so an answer from before can't look current after.
    pub fn all_disconnected(&mut self) {
        self.entries.clear();
        self.held.clear();
        self.rev += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::identity::Keypair;
    use reamesh_relay_proto::sessions::{SessionEntry, seal_entry};

    type List = SessionList<u32>;

    fn conn() -> ConnectionId {
        ConnectionId::new_unchecked(1)
    }

    fn peer_with_entry() -> (PeerId, Vec<u8>) {
        let key = Keypair::generate_ed25519();
        let owner = key.public().to_peer_id();
        let relay = PeerId::random();
        let entry = SessionEntry {
            v: SessionEntry::VERSION,
            relay: relay.to_string(),
            session_id: "abc".into(),
            session_name: "Song".into(),
            owner: owner.to_string(),
            owner_name: String::new(),
            owner_color: String::new(),
            auth: false,
            salt: String::new(),
            files: 0,
            bytes: 0,
            proto: 0,
            ver: String::new(),
            ticket: format!("reamesh://join/abc?p={owner}&r={relay}~203.0.113.5~1~1"),
            extra: Default::default(),
        };
        (owner, seal_entry(&key, &entry).unwrap())
    }

    fn publish(l: &mut List, peer: PeerId, env: &[u8]) -> PublishResponse {
        l.publish(peer, Access::Key("band"), b"proof", Some(env.to_vec()), true)
    }

    fn reason(r: &PublishResponse) -> Option<&str> {
        r.reason.as_deref()
    }

    #[test]
    fn an_off_list_refuses_and_turning_it_off_forgets() {
        let mut l = List::new(false);
        let (host, env) = peer_with_entry();
        assert_eq!(reason(&publish(&mut l, host, &env)), Some(REASON_DISABLED));
        l.set_enabled(true);
        assert!(publish(&mut l, host, &env).ok);
        assert_eq!(l.len(), 1);
        l.set_enabled(false);
        assert_eq!(l.len(), 0);
        let got = l.query(PeerId::random(), conn(), Access::Key("x"), b"p", 0, 1, Instant::now());
        assert!(got[0].1.ok && !got[0].1.enabled && got[0].1.entries.is_empty());
    }

    #[test]
    fn only_a_reserved_host_with_its_own_signature_may_publish() {
        let mut l = List::new(true);
        let (host, env) = peer_with_entry();
        let r = l.publish(host, Access::Key("band"), b"p", Some(env.clone()), false);
        assert_eq!(reason(&r), Some(REASON_NOT_RESERVED));
        let r = l.publish(host, Access::BadKey, b"p", Some(env.clone()), true);
        assert_eq!(reason(&r), Some(REASON_BAD_KEY));
        let r = l.publish(host, Access::Banned, b"p", Some(env.clone()), true);
        assert_eq!(reason(&r), Some(REASON_BANNED));
        let r = l.publish(host, Access::Expired, b"p", Some(env.clone()), true);
        assert_eq!(reason(&r), Some(REASON_EXPIRED));
        // Someone else's entry, as this peer's own.
        let r = publish(&mut l, PeerId::random(), &env);
        assert_eq!(reason(&r), Some(REASON_BAD_ENTRY));
        let r = publish(&mut l, host, &vec![0; ENTRY_MAX_BYTES + 1]);
        assert_eq!(reason(&r), Some(REASON_TOO_LARGE));
        assert!(l.is_empty());
        assert!(publish(&mut l, host, &env).ok);
        assert!(l.is_listed(&host));
    }

    #[test]
    fn a_refused_viewer_gets_no_entries() {
        let mut l = List::new(true);
        let (host, env) = peer_with_entry();
        publish(&mut l, host, &env);
        let now = Instant::now();
        for (access, want) in [
            (Access::BadKey, REASON_BAD_KEY),
            (Access::Banned, REASON_BANNED),
            (Access::Expired, REASON_EXPIRED),
        ] {
            let got = l.query(PeerId::random(), conn(), access, b"p", 0, 1, now);
            assert!(!got[0].1.ok);
            assert_eq!(got[0].1.reason.as_deref(), Some(want));
            assert!(got[0].1.entries.is_empty());
            assert_eq!(got[0].1.rev, 0);
        }
        assert_eq!(l.watching(), 0);
    }

    #[test]
    fn a_request_with_the_current_rev_waits_for_a_change() {
        let mut l = List::new(true);
        let (host, env) = peer_with_entry();
        let viewer = PeerId::random();
        let now = Instant::now();
        let first = l.query(viewer, conn(), Access::Key("x"), b"p", 0, 1, now);
        assert_eq!(first.len(), 1);
        let rev = first[0].1.rev;
        assert!(l.query(viewer, conn(), Access::Key("x"), b"p", rev, 2, now).is_empty());
        assert_eq!(l.watching(), 1);
        assert!(l.due(now, |_| true).is_empty());
        publish(&mut l, host, &env);
        let due = l.due(now, |_| true);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].0, 2);
        assert_eq!(due[0].1.entries.len(), 1);
        assert_eq!(due[0].1.entries[0].key_label, "band");
        assert!(due[0].1.rev > rev);
        assert_eq!(l.watching(), 0);
    }

    #[test]
    fn republishing_the_same_entry_wakes_nobody() {
        let mut l = List::new(true);
        let (host, env) = peer_with_entry();
        publish(&mut l, host, &env);
        let rev = l.answer().rev;
        publish(&mut l, host, &env);
        assert_eq!(l.answer().rev, rev);
    }

    #[test]
    fn a_hold_ends_after_its_time_and_a_gone_viewer_is_forgotten() {
        let mut l = List::new(true);
        let t0 = Instant::now();
        let rev = l.answer().rev;
        let a = PeerId::random();
        assert!(l.query(a, conn(), Access::Key("x"), b"p", rev, 1, t0).is_empty());
        let late = t0 + Duration::from_secs(SESSIONS_HOLD_SECS);
        let due = l.due(late, |_| true);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].1.rev, rev);
        assert!(l.query(a, conn(), Access::Key("x"), b"p", rev, 2, t0).is_empty());
        assert!(l.due(t0, |_| false).is_empty());
        assert_eq!(l.watching(), 0);
    }

    #[test]
    fn a_second_request_from_one_peer_replaces_the_first() {
        let mut l = List::new(true);
        let now = Instant::now();
        let rev = l.answer().rev;
        let a = PeerId::random();
        assert!(l.query(a, conn(), Access::Key("x"), b"p", rev, 1, now).is_empty());
        let got = l.query(a, conn(), Access::Key("x"), b"p", rev, 2, now);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, 1);
        assert_eq!(l.watching(), 1);
        // Another connection of the same peer waits beside it, or the two
        // would keep waking each other.
        let other = ConnectionId::new_unchecked(2);
        assert!(l.query(a, other, Access::Key("x"), b"p", rev, 3, now).is_empty());
        assert_eq!(l.watching(), 2);
    }

    #[test]
    fn revocation_drops_entries_and_ends_held_requests() {
        let mut l = List::new(true);
        let (gone_host, gone_env) = peer_with_entry();
        let (kept_host, kept_env) = peer_with_entry();
        l.publish(gone_host, Access::Key("gone"), b"gone", Some(gone_env), true);
        l.publish(kept_host, Access::Key("kept"), b"kept", Some(kept_env), true);
        let now = Instant::now();
        let rev = l.answer().rev;
        let (gone_viewer, kept_viewer) = (PeerId::random(), PeerId::random());
        assert!(l.query(gone_viewer, conn(), Access::Key("gone"), b"gone", rev, 1, now).is_empty());
        assert!(l.query(kept_viewer, conn(), Access::Key("kept"), b"kept", rev, 2, now).is_empty());
        let answers = l.recheck(|_, proof| {
            if proof == b"gone" { Access::BadKey } else { Access::Key("kept") }
        });
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].0, 1);
        assert_eq!(answers[0].1.reason.as_deref(), Some(REASON_BAD_KEY));
        assert_eq!(l.len(), 1);
        assert!(l.is_listed(&kept_host));
        // The other viewer learns on the next tick that an entry went.
        let due = l.due(now, |_| true);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].1.entries.len(), 1);
    }

    #[test]
    fn a_rebuild_forgets_everything_and_rev_goes_on() {
        let mut l = List::new(true);
        let (host, env) = peer_with_entry();
        publish(&mut l, host, &env);
        let rev = l.answer().rev;
        l.all_disconnected();
        assert!(l.is_empty());
        assert!(l.answer().rev > rev);
    }

    #[test]
    fn the_used_up_budget_is_news() {
        let mut l = List::new(true);
        let rev = l.answer().rev;
        l.set_budget_used(true);
        assert!(l.answer().budget_used);
        assert!(l.answer().rev > rev);
    }
}
