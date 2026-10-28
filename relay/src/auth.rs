//! Which peers may hold a reservation: filled by the auth handler (see
//! `reamesh-relay-proto` for the protocol), read by the relay's reservation limiter.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use libp2p::PeerId;

/// How long an authorization outlives the host's last connection, so a host
/// that reconnects after a network switch can reserve again straight away.
pub const GRACE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
struct Entry {
    label: String,
    connections: usize,
    /// Set once the last connection closed.
    expires: Option<Instant>,
}

/// Which peers may hold a reservation, shared between the auth handler and
/// the relay's reservation limiter.
#[derive(Debug, Default)]
pub struct AuthState {
    entries: HashMap<PeerId, Entry>,
    /// Peers with an active reservation (the relay's hosts).
    pub reserved: HashSet<PeerId>,
    pub banned: HashSet<PeerId>,
    pub max_hosts: usize,
}

impl AuthState {
    pub fn authorize(&mut self, peer: PeerId, label: &str, connections: usize) {
        self.entries.insert(
            peer,
            Entry {
                label: label.to_owned(),
                connections: connections.max(1),
                expires: None,
            },
        );
    }

    pub fn is_authorized(&self, peer: &PeerId, now: Instant) -> bool {
        !self.banned.contains(peer)
            && self
                .entries
                .get(peer)
                .is_some_and(|e| e.connections > 0 || e.expires.is_some_and(|t| t > now))
    }

    pub fn label(&self, peer: &PeerId) -> Option<&str> {
        self.entries.get(peer).map(|e| e.label.as_str())
    }

    /// No room for one more host (an existing host renewing always fits).
    pub fn is_full(&self, peer: &PeerId) -> bool {
        !self.reserved.contains(peer) && self.reserved.len() >= self.max_hosts
    }

    /// The reservation limiter's verdict.
    pub fn may_reserve(&self, peer: &PeerId, now: Instant) -> bool {
        self.is_authorized(peer, now) && !self.is_full(peer)
    }

    pub fn connection_count(&mut self, peer: &PeerId, connections: usize, now: Instant) {
        if let Some(e) = self.entries.get_mut(peer) {
            e.connections = connections;
            e.expires = (connections == 0).then(|| now + GRACE);
        }
        if connections == 0 {
            self.reserved.remove(peer);
        }
    }

    /// Every connection dropped at once (the swarm is being rebuilt).
    pub fn all_disconnected(&mut self, now: Instant) {
        for e in self.entries.values_mut() {
            if e.connections > 0 {
                e.connections = 0;
                e.expires = Some(now + GRACE);
            }
        }
        self.reserved.clear();
    }

    pub fn purge(&mut self, now: Instant) {
        self.entries
            .retain(|_, e| e.connections > 0 || e.expires.is_some_and(|t| t > now));
    }

    /// Drops every authorization whose key label is no longer configured or
    /// whose peer is now banned; returns the peers to disconnect.
    pub fn revoke(&mut self, labels: &HashSet<String>) -> Vec<PeerId> {
        let gone: Vec<PeerId> = self
            .entries
            .iter()
            .filter(|(p, e)| !labels.contains(&e.label) || self.banned.contains(p))
            .map(|(p, _)| *p)
            .collect();
        for p in &gone {
            self.entries.remove(p);
            self.reserved.remove(p);
        }
        gone
    }

    /// Sorted, so `status.json` doesn't reorder its hosts on every write.
    pub fn hosts(&self) -> Vec<(PeerId, String)> {
        let mut v: Vec<_> = self
            .reserved
            .iter()
            .map(|p| (*p, self.label(p).unwrap_or("?").to_owned()))
            .collect();
        v.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::identity::Keypair;

    fn peer() -> PeerId {
        Keypair::generate_ed25519().public().to_peer_id()
    }

    #[test]
    fn authorization_survives_a_short_disconnect_only() {
        let mut s = AuthState {
            max_hosts: 1,
            ..Default::default()
        };
        let (host, other) = (peer(), peer());
        let t0 = Instant::now();
        assert!(!s.may_reserve(&host, t0));
        s.authorize(host, "default", 1);
        assert!(s.may_reserve(&host, t0));
        s.connection_count(&host, 0, t0);
        assert!(s.may_reserve(&host, t0 + GRACE / 2));
        assert!(!s.may_reserve(&host, t0 + GRACE * 2));
        s.purge(t0 + GRACE * 2);
        assert!(s.label(&host).is_none());

        // Full: a second host can't reserve while one holds the only slot.
        s.authorize(host, "default", 1);
        s.authorize(other, "default", 1);
        s.reserved.insert(host);
        assert!(s.may_reserve(&host, t0));
        assert!(!s.may_reserve(&other, t0));
    }

    #[test]
    fn revoke_drops_removed_labels_and_bans() {
        let mut s = AuthState {
            max_hosts: 8,
            ..Default::default()
        };
        let (a, b, c) = (peer(), peer(), peer());
        s.authorize(a, "keep", 1);
        s.authorize(b, "gone", 1);
        s.authorize(c, "keep", 1);
        s.banned.insert(c);
        let labels: HashSet<String> = ["keep".to_owned()].into();
        let mut gone = s.revoke(&labels);
        gone.sort();
        let mut want = vec![b, c];
        want.sort();
        assert_eq!(gone, want);
        assert!(s.is_authorized(&a, Instant::now()));
    }
}
