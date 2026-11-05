//! `status.json`: the running relay rewrites it every few seconds;
//! `reamesh-relay status` reads it. A file instead of a socket: nothing
//! listens beyond the relay ports, and the data dir's permissions already
//! decide who may read it.

use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::{Mode, STATUS_FILE};
use crate::term::{self, Style};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub running: bool,
    pub pid: u32,
    /// Unix seconds of this write.
    pub updated: u64,
    pub started: u64,
    pub version: String,
    pub peer_id: String,
    /// The mode in effect (coordination once the budget is used up).
    pub mode: Mode,
    pub configured_mode: Mode,
    pub budget_exhausted: bool,
    pub month: String,
    pub sent: u64,
    pub received: u64,
    /// Bytes per month; 0 = unlimited.
    pub budget: u64,
    pub hosts: Vec<HostStatus>,
    pub circuits: usize,
    /// Peers with an open connection (hosts and guests).
    pub peers: usize,
    /// Own public IPs as hosts saw them.
    pub observed_ips: Vec<IpAddr>,
    pub listening: Vec<String>,
    /// Bytes sent per second over the last few seconds.
    #[serde(default)]
    pub rate: u64,
    /// Connections that came in over each transport since the start.
    #[serde(default)]
    pub inbound_tcp: Inbound,
    #[serde(default)]
    pub inbound_quic: Inbound,
    /// The session list: whether it is on, how many sessions are on it, how
    /// many viewers wait for changes. Counts only, never a session name.
    #[serde(default)]
    pub session_list: bool,
    #[serde(default)]
    pub listed: usize,
    #[serde(default)]
    pub watching: usize,
    /// This month's traffic per key.
    #[serde(default)]
    pub keys: Vec<KeyUsage>,
    /// The operator's notice as one line, while it applies.
    #[serde(default)]
    pub notice: Option<String>,
    /// A newer version, as one line.
    #[serde(default)]
    pub update: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyUsage {
    pub label: String,
    /// Bytes sent this month for its hosts and their guests.
    pub sent: u64,
    /// Its own monthly budget in bytes, 0 for none.
    pub budget: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inbound {
    pub count: u64,
    /// Unix seconds of the newest one.
    pub last: u64,
}

/// After this many connections over TCP and none over QUIC, UDP is most
/// likely blocked: ReaMesh dials both at once and QUIC, with fewer round
/// trips, usually wins. The other way round means nothing, so TCP is never
/// suspected.
pub const QUIC_SUSPECT_AFTER: u64 = 5;

/// Rates below this are the relay's own small protocols, not traffic.
const RATE_SHOWN_FROM: u64 = 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostStatus {
    /// The label of the key it authenticated with.
    pub label: String,
    pub peer_id: String,
    /// The key's fingerprint, as `key list` shows it.
    pub fingerprint: String,
    /// Unix seconds since it holds its reservation.
    pub since: u64,
    /// Relayed connections to it right now.
    pub guests: usize,
    /// Whether it has an entry on the session list.
    #[serde(default)]
    pub listed: bool,
}

/// A status older than this means the relay is gone (it writes every 5s).
pub const STALE_AFTER_SECS: u64 = 30;

impl Status {
    pub fn read(dir: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(dir.join(STATUS_FILE)).ok()?).ok()
    }

    pub fn write(&self, dir: &Path) -> std::io::Result<()> {
        let path = dir.join(STATUS_FILE);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }

    pub fn is_live(&self, now: u64) -> bool {
        self.running && now.saturating_sub(self.updated) <= STALE_AFTER_SECS
    }

    /// What `reamesh-relay status` prints for a running relay.
    pub fn render(&self, now: u64, s: Style) -> String {
        let mode = match self.mode {
            Mode::Full => "full",
            Mode::Coordination => "coordination",
        };
        let up = term::duration(Duration::from_secs(now.saturating_sub(self.started)));
        let dot = if s.rich {
            format!("{} ", s.strong("●"))
        } else {
            String::new()
        };
        let mut out = format!(
            "{dot}{} {} {up}   {} {}   {} {}\n\n",
            s.strong("running"),
            s.dim("for"),
            s.dim("mode"),
            s.strong(mode),
            s.dim("version"),
            self.version,
        );
        let mut traffic = format!("{} this month", term::bytes(self.sent));
        if self.rate >= RATE_SHOWN_FROM {
            traffic += &format!(", now {}/s", term::bytes(self.rate));
        }
        out += &s.field("traffic", &format!("{traffic}\n"));
        if self.budget == 0 {
            out += &s.field("budget", "none\n");
        } else {
            let used = self.sent as f64 / self.budget as f64;
            let pct = match used * 100.0 {
                p if p > 0.0 && p < 1.0 => "<1".to_owned(),
                p => format!("{:.0}", p.min(100.0).floor()),
            };
            let mut line = format!("{pct} % of {}", term::bytes(self.budget));
            let mut warn = false;
            if self.budget_exhausted {
                line += &format!(
                    ", coordination only until {}",
                    term::next_month_start(&self.month)
                );
            } else {
                match self.projection(now) {
                    Some(Projection::UsedUp(at)) => {
                        line += &format!(", used up around {} at this pace", term::utc_day(at));
                        warn = true;
                    }
                    Some(Projection::Total(total, last)) => {
                        line +=
                            &format!(", about {} by {}", term::bytes(total), term::utc_day(last));
                    }
                    None => {}
                }
            }
            let line = if warn { s.amber(&line) } else { line };
            let line = if s.rich {
                let bar = term::bar(used, 20);
                let bar = if used >= 0.8 || warn {
                    s.amber(&bar)
                } else {
                    bar
                };
                format!("{bar}  {line}")
            } else {
                line
            };
            out += &s.field("budget", &format!("{line}\n"));
        }
        out += &s.field("inbound", &format!("{}\n", self.inbound_line(now, s)));
        out += &s.field("list", &format!("{}\n", self.list_line()));
        if let Some(notice) = &self.notice {
            out += &s.field("notice", &format!("{notice}\n"));
        }
        if let Some(update) = &self.update {
            out += &s.field("update", &format!("{update}\n"));
        }
        out.push('\n');
        if self.hosts.is_empty() {
            out += &s.dim("No hosts connected.\n");
            return out + &self.keys_table(s);
        }
        let rows: Vec<Vec<String>> = self
            .hosts
            .iter()
            .map(|h| {
                let shared = self.hosts.iter().filter(|o| o.label == h.label).count() > 1;
                let name = if shared {
                    let id = &h.peer_id[h.peer_id.len().saturating_sub(6)..];
                    format!("{} {}{id}", h.label, s.sym("…", "..."))
                } else {
                    h.label.clone()
                };
                let guests = match h.guests {
                    0 => "none".to_owned(),
                    1 => "1 guest".to_owned(),
                    n => format!("{n} guests"),
                };
                let mut row = vec![
                    name,
                    h.fingerprint.clone(),
                    term::duration(Duration::from_secs(now.saturating_sub(h.since))),
                    guests,
                ];
                if self.session_list {
                    row.push(if h.listed { "yes" } else { "no" }.to_owned());
                }
                row
            })
            .collect();
        let head: &[&str] = if self.session_list {
            &["HOST", "KEY", "ONLINE", "RELAYED", "LISTED"]
        } else {
            &["HOST", "KEY", "ONLINE", "RELAYED"]
        };
        out + &term::table(s, head, &rows) + &self.keys_table(s)
    }

    /// Who used how much of this month's traffic: only once there is
    /// something to tell.
    fn keys_table(&self, s: Style) -> String {
        if self.keys.iter().all(|k| k.sent == 0 && k.budget == 0) {
            return String::new();
        }
        let rows: Vec<Vec<String>> = self
            .keys
            .iter()
            .map(|k| {
                let budget = if k.budget == 0 {
                    "-".to_owned()
                } else {
                    let used = crate::traffic::percent_of(k.sent, k.budget);
                    format!("{used} % of {}", term::bytes(k.budget))
                };
                vec![k.label.clone(), term::bytes(k.sent), budget]
            })
            .collect();
        format!(
            "\n{}",
            term::table(s, &["KEY", "THIS MONTH", "OWN BUDGET"], &rows)
        )
    }
}

enum Projection {
    /// The budget runs out at this Unix time.
    UsedUp(u64),
    /// The month ends with this much sent; its last second.
    Total(u64, u64),
}

impl Status {
    /// Where this month ends up at the pace so far, once a day of it has
    /// passed (before that, one evening's session would set the pace).
    fn projection(&self, now: u64) -> Option<Projection> {
        let (start, end) = term::month_bounds(&self.month)?;
        let elapsed = now
            .checked_sub(start)
            .filter(|e| *e >= 86_400 && now < end)?;
        if self.sent == 0 {
            return None;
        }
        let total = u128::from(self.sent) * u128::from(end - start) / u128::from(elapsed);
        if self.budget > 0 && total > u128::from(self.budget) {
            let at = u128::from(start)
                + u128::from(self.budget) * u128::from(elapsed) / u128::from(self.sent);
            return Some(Projection::UsedUp(u64::try_from(at).ok()?));
        }
        Some(Projection::Total(u64::try_from(total).ok()?, end - 1))
    }

    /// "TCP 12 s ago   QUIC none yet", QUIC in amber once only TCP gets in.
    fn inbound_line(&self, now: u64, s: Style) -> String {
        let when = |i: Inbound| {
            if i.count == 0 {
                "none yet".to_owned()
            } else {
                let ago = Duration::from_secs(now.saturating_sub(i.last));
                format!("{} ago", term::duration(ago))
            }
        };
        let quic = format!("QUIC {}", when(self.inbound_quic));
        let quic = if self.quic_suspect() {
            s.amber(&quic)
        } else {
            quic
        };
        format!("TCP {}   {quic}", when(self.inbound_tcp))
    }

    /// "on, 2 sessions, 3 watching", or "off".
    fn list_line(&self) -> String {
        if !self.session_list {
            return "off".to_owned();
        }
        let plural = |n: usize, one: &str| {
            if n == 1 {
                format!("1 {one}")
            } else {
                format!("{n} {one}s")
            }
        };
        let mut line = format!("on, {}", plural(self.listed, "session"));
        if self.watching > 0 {
            line += &format!(", {} watching", self.watching);
        }
        line
    }

    pub fn quic_suspect(&self) -> bool {
        self.inbound_tcp.count >= QUIC_SUSPECT_AFTER && self.inbound_quic.count == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(label: &str, id: &str, guests: usize) -> HostStatus {
        HostStatus {
            label: label.into(),
            peer_id: id.into(),
            fingerprint: "ec24d153".into(),
            since: 1000,
            guests,
            listed: false,
        }
    }

    fn status() -> Status {
        Status {
            running: true,
            pid: 1,
            updated: 9000,
            started: 1000,
            version: "0.1.0".into(),
            peer_id: String::new(),
            mode: Mode::Full,
            configured_mode: Mode::Full,
            budget_exhausted: false,
            month: "2026-10".into(),
            sent: 5_000_000_000,
            received: 0,
            budget: 500_000_000_000,
            hosts: Vec::new(),
            circuits: 0,
            peers: 0,
            observed_ips: Vec::new(),
            listening: Vec::new(),
            rate: 0,
            inbound_tcp: Inbound::default(),
            inbound_quic: Inbound::default(),
            keys: Vec::new(),
            notice: None,
            update: None,
            session_list: false,
            listed: 0,
            watching: 0,
        }
    }

    #[test]
    fn budget_line_projects_the_month() {
        // Ten days into November 2026.
        let start = 1_793_491_200;
        let now = start + 10 * 86_400;
        let mut st = status();
        st.month = "2026-11".into();
        st.sent = 100_000_000_000;
        st.rate = 1_400_000;
        let out = st.render(now, Style::plain());
        assert!(out.contains("traffic   100 GB this month, now 1.4 MB/s\n"));
        assert!(out.contains("budget    20 % of 500 GB, about 300 GB by Nov 30\n"));
        st.sent = 200_000_000_000;
        let out = st.render(now, Style::plain());
        assert!(out.contains("40 % of 500 GB, used up around Nov 26 at this pace\n"));
        // Too early in the month to tell.
        let out = st.render(start + 3600, Style::plain());
        assert!(out.contains("budget    40 % of 500 GB\n"));
    }

    #[test]
    fn status_shows_the_notice_and_what_each_key_used() {
        let mut st = status();
        let out = st.render(9000, Style::plain());
        assert!(!out.contains("notice"));
        assert!(!out.contains("THIS MONTH"));
        st.notice = Some("Serverumzug".into());
        st.keys = vec![
            KeyUsage {
                label: "band".into(),
                sent: 3_000_000_000,
                budget: 0,
            },
            KeyUsage {
                label: "lena".into(),
                sent: 2_000_000_000,
                budget: 50_000_000_000,
            },
        ];
        let out = st.render(9000, Style::plain());
        assert!(out.contains("notice    Serverumzug\n"), "{out}");
        assert!(out.contains("band   3 GB"), "{out}");
        assert!(out.contains("4 % of 50 GB"), "{out}");
        // Also while no host is online; that is when it gets read.
        assert!(out.contains("No hosts connected."));
    }

    #[test]
    fn inbound_line_suspects_only_quic() {
        let mut st = status();
        st.inbound_tcp = Inbound {
            count: 2,
            last: 8988,
        };
        let out = st.render(9000, Style::plain());
        assert!(out.contains("inbound   TCP 12 s ago   QUIC none yet\n"));
        assert!(!st.quic_suspect());
        st.inbound_tcp.count = QUIC_SUSPECT_AFTER;
        assert!(st.quic_suspect());
        st.inbound_quic = Inbound {
            count: 1,
            last: 8000,
        };
        assert!(!st.quic_suspect());
        let tcp_only = Status {
            inbound_tcp: Inbound::default(),
            ..st.clone()
        };
        assert!(!tcp_only.quic_suspect());
    }

    #[test]
    fn the_list_shows_as_counts_and_a_column_only_when_on() {
        let mut st = status();
        st.hosts = vec![host("lena", "x", 1)];
        let off = st.render(9000, Style::plain());
        assert!(off.contains("list      off\n"));
        assert!(!off.contains("LISTED"));
        st.session_list = true;
        st.listed = 2;
        st.watching = 3;
        st.hosts[0].listed = true;
        let on = st.render(9000, Style::plain());
        assert!(on.contains("list      on, 2 sessions, 3 watching\n"));
        assert!(on.contains("LISTED"));
        assert!(on.contains("1 guest   yes"));
        st.listed = 1;
        st.watching = 0;
        assert!(st.render(9000, Style::plain()).contains("list      on, 1 session\n"));
    }

    #[test]
    fn plain_status_names_hosts_by_key() {
        let st = Status {
            hosts: vec![
                host("band", "12D3KooWaaaaaa", 1),
                host("band", "12D3KooWbbbbbb", 0),
                host("lena", "x", 2),
            ],
            circuits: 3,
            peers: 6,
            ..status()
        };
        let out = st.render(9000, Style::plain());
        assert!(out.starts_with("running for 2 h 13 min   mode full   version 0.1.0\n"));
        assert!(out.contains("budget    1 % of 500 GB\n"));
        assert!(out.contains("band ...aaaaaa   ec24d153   2 h 13 min   1 guest\n"));
        assert!(out.contains("lena             ec24d153   2 h 13 min   2 guests\n"));
        assert!(!out.contains('\x1b'));
    }
}
