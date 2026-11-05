//! Byte counting for the monthly budget: a muxer wrapper counts what every
//! substream reads and writes (relayed data plus the small protocols), and a
//! ledger keeps the month's total in `traffic.json`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{SystemTime, UNIX_EPOCH};

use futures::{AsyncRead, AsyncWrite};
use libp2p::PeerId;
use libp2p::core::muxing::{StreamMuxer, StreamMuxerBox, StreamMuxerEvent, SubstreamBox};
use serde::{Deserialize, Serialize};

use crate::config::Mode;

#[derive(Debug, Default)]
pub struct Counters {
    pub sent: AtomicU64,
    pub received: AtomicU64,
    /// The same per peer, to tell which key the traffic belongs to.
    peers: Mutex<HashMap<PeerId, Arc<PeerCount>>>,
}

#[derive(Debug, Default)]
pub struct PeerCount {
    sent: AtomicU64,
    received: AtomicU64,
}

impl Counters {
    /// Bytes counted since the last call.
    pub fn take(&self) -> (u64, u64) {
        (
            self.sent.swap(0, Ordering::Relaxed),
            self.received.swap(0, Ordering::Relaxed),
        )
    }

    pub fn peer(&self, peer: PeerId) -> Arc<PeerCount> {
        self.peers.lock().unwrap().entry(peer).or_default().clone()
    }

    /// Bytes per peer since the last call, peers with none left out. A peer
    /// whose connections are all gone drops out of the table here.
    pub fn take_peers(&self) -> Vec<(PeerId, u64, u64)> {
        let mut peers = self.peers.lock().unwrap();
        let out = peers
            .iter()
            .map(|(p, c)| {
                (
                    *p,
                    c.sent.swap(0, Ordering::Relaxed),
                    c.received.swap(0, Ordering::Relaxed),
                )
            })
            .filter(|(_, s, r)| *s != 0 || *r != 0)
            .collect();
        peers.retain(|_, c| Arc::strong_count(c) > 1);
        out
    }
}

pub struct CountingMuxer {
    inner: StreamMuxerBox,
    counters: Arc<Counters>,
    peer: Arc<PeerCount>,
}

impl CountingMuxer {
    pub fn new(inner: StreamMuxerBox, counters: Arc<Counters>, peer: PeerId) -> Self {
        let count = counters.peer(peer);
        Self {
            inner,
            counters,
            peer: count,
        }
    }

    fn wrap(&self, s: SubstreamBox) -> CountingStream {
        CountingStream {
            inner: s,
            counters: self.counters.clone(),
            peer: self.peer.clone(),
        }
    }
}

impl StreamMuxer for CountingMuxer {
    type Substream = CountingStream;
    type Error = io::Error;

    fn poll_inbound(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Self::Substream, Self::Error>> {
        let s = futures::ready!(Pin::new(&mut self.inner).poll_inbound(cx))?;
        Poll::Ready(Ok(self.wrap(s)))
    }

    fn poll_outbound(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Self::Substream, Self::Error>> {
        let s = futures::ready!(Pin::new(&mut self.inner).poll_outbound(cx))?;
        Poll::Ready(Ok(self.wrap(s)))
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<StreamMuxerEvent, Self::Error>> {
        Pin::new(&mut self.inner).poll(cx)
    }
}

pub struct CountingStream {
    inner: SubstreamBox,
    counters: Arc<Counters>,
    peer: Arc<PeerCount>,
}

impl AsyncRead for CountingStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let n = futures::ready!(Pin::new(&mut self.inner).poll_read(cx, buf))?;
        self.counters
            .received
            .fetch_add(n as u64, Ordering::Relaxed);
        self.peer.received.fetch_add(n as u64, Ordering::Relaxed);
        Poll::Ready(Ok(n))
    }
}

impl AsyncWrite for CountingStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let n = futures::ready!(Pin::new(&mut self.inner).poll_write(cx, buf))?;
        self.counters.sent.fetch_add(n as u64, Ordering::Relaxed);
        self.peer.sent.fetch_add(n as u64, Ordering::Relaxed);
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

/// This month's traffic, persisted so a restart doesn't reset the budget.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    /// `YYYY-MM`, UTC.
    pub month: String,
    pub sent: u64,
    pub received: u64,
    /// Bytes sent this month on behalf of each key's hosts and their guests.
    #[serde(default)]
    pub keys: BTreeMap<String, u64>,
    /// The alerts that went out this month, so a restart doesn't repeat them.
    #[serde(default)]
    pub alerted: BTreeSet<String>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Ledger {
    pub fn load(path: &Path) -> Self {
        let mut l: Ledger = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        l.path = Some(path.to_owned());
        l.roll(&current_month());
        l
    }

    pub fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }

    /// Starts over when `month` differs; returns whether it did.
    pub fn roll(&mut self, month: &str) -> bool {
        if self.month == month {
            return false;
        }
        self.month = month.to_owned();
        self.sent = 0;
        self.received = 0;
        self.keys.clear();
        self.alerted.clear();
        true
    }

    pub fn add(&mut self, sent: u64, received: u64) {
        self.sent = self.sent.saturating_add(sent);
        self.received = self.received.saturating_add(received);
    }

    pub fn add_key(&mut self, label: &str, sent: u64) {
        let total = self.keys.entry(label.to_owned()).or_default();
        *total = total.saturating_add(sent);
    }

    pub fn key_sent(&self, label: &str) -> u64 {
        self.keys.get(label).copied().unwrap_or(0)
    }

    /// True the first time this month, for alerts that go out once.
    pub fn first_alert(&mut self, what: &str) -> bool {
        self.alerted.insert(what.to_owned())
    }

    pub fn exhausted(&self, budget: u64) -> bool {
        budget > 0 && self.sent >= budget
    }

    /// The mode the relay runs in: the configured one, or coordination once
    /// the month's budget is used up.
    pub fn effective_mode(&self, configured: Mode, budget: u64) -> Mode {
        if self.exhausted(budget) {
            Mode::Coordination
        } else {
            configured
        }
    }
}

/// Whole percent of a budget used, 0 without a budget, at most 100.
pub fn percent_of(used: u64, budget: u64) -> u8 {
    if budget == 0 {
        return 0;
    }
    (u128::from(used) * 100 / u128::from(budget)).min(100) as u8
}

pub fn current_month() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    month_of(secs)
}

/// `YYYY-MM` (UTC) of a Unix timestamp.
pub fn month_of(unix_secs: u64) -> String {
    // Howard Hinnant's civil_from_days.
    let z = (unix_secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months() {
        assert_eq!(month_of(0), "1970-01");
        assert_eq!(month_of(951_782_400), "2000-02"); // 2000-02-29
        assert_eq!(month_of(1_798_761_599), "2026-12"); // 2026-12-31 23:59:59
        assert_eq!(month_of(1_798_761_600), "2027-01");
    }

    #[test]
    fn percent_of_a_budget() {
        assert_eq!(percent_of(5, 0), 0);
        assert_eq!(percent_of(899, 1000), 89);
        assert_eq!(percent_of(5000, 1000), 100);
        assert_eq!(percent_of(u64::MAX, u64::MAX), 100);
    }

    #[test]
    fn budget_switches_to_coordination_and_resets_monthly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("traffic.json");
        let mut l = Ledger::load(&path);
        let budget = 1_000;
        assert_eq!(l.effective_mode(Mode::Full, budget), Mode::Full);
        l.add(999, 5_000);
        assert_eq!(l.effective_mode(Mode::Full, budget), Mode::Full);
        l.add(1, 0);
        assert_eq!(l.effective_mode(Mode::Full, budget), Mode::Coordination);
        // No budget: never switches.
        assert_eq!(l.effective_mode(Mode::Full, 0), Mode::Full);
        // Configured coordination stays coordination.
        let fresh = Ledger::default();
        assert_eq!(
            fresh.effective_mode(Mode::Coordination, budget),
            Mode::Coordination
        );

        // The counter survives a restart within the month.
        l.save().unwrap();
        let again = Ledger::load(&path);
        assert_eq!((again.sent, again.received), (1_000, 5_000));
        assert!(again.exhausted(budget));

        // A new month starts from zero, the keys and alerts too.
        let mut next = again.clone();
        next.add_key("lena", 5);
        assert!(next.first_alert("budget"));
        assert!(!next.first_alert("budget"));
        assert_eq!(next.key_sent("lena"), 5);
        assert!(next.roll("2999-01"));
        assert_eq!(next.key_sent("lena"), 0);
        assert!(next.first_alert("budget"));
        assert!(!next.roll("2999-01"));
        assert_eq!(next.effective_mode(Mode::Full, budget), Mode::Full);
    }
}
