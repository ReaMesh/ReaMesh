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
}

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
            "{dot}{}  {} {up}   {} {}   {} {}\n\n",
            s.strong("running"),
            s.dim("for"),
            s.dim("mode"),
            s.strong(mode),
            s.dim("version"),
            self.version,
        );
        out += &s.field(
            "traffic",
            &format!("{} this month\n", term::bytes(self.sent)),
        );
        if self.budget == 0 {
            out += &s.field("budget", "none\n");
        } else {
            let used = self.sent as f64 / self.budget as f64;
            let pct = match used * 100.0 {
                p if p > 0.0 && p < 1.0 => "<1".to_owned(),
                p => format!("{:.0}", p.min(100.0).floor()),
            };
            let mut line = format!("{pct} % of {}", term::bytes(self.budget));
            if self.budget_exhausted {
                line += &format!(
                    ", coordination only until {}",
                    term::next_month_start(&self.month)
                );
            }
            let line = if s.rich {
                let bar = term::bar(used, 20);
                let bar = if used >= 0.8 { s.amber(&bar) } else { bar };
                format!("{bar}  {line}")
            } else {
                line
            };
            out += &s.field("budget", &format!("{line}\n"));
        }
        out.push('\n');
        if self.hosts.is_empty() {
            out += &s.dim("No hosts connected.\n");
            return out;
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
                vec![
                    name,
                    h.fingerprint.clone(),
                    term::duration(Duration::from_secs(now.saturating_sub(h.since))),
                    guests,
                ]
            })
            .collect();
        out + &term::table(s, &["HOST", "KEY", "ONLINE", "RELAYED"], &rows)
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
        }
    }

    #[test]
    fn plain_status_names_hosts_by_key() {
        let st = Status {
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
            hosts: vec![
                host("band", "12D3KooWaaaaaa", 1),
                host("band", "12D3KooWbbbbbb", 0),
                host("lena", "x", 2),
            ],
            circuits: 3,
            peers: 6,
            observed_ips: Vec::new(),
            listening: Vec::new(),
        };
        let out = st.render(9000, Style::plain());
        assert!(out.starts_with("running  for 2 h 13 min   mode full   version 0.1.0\n"));
        assert!(out.contains("budget    1 % of 500 GB\n"));
        assert!(out.contains("band ...aaaaaa   ec24d153   2 h 13 min   1 guest\n"));
        assert!(out.contains("lena             ec24d153   2 h 13 min   2 guests\n"));
        assert!(!out.contains('\x1b'));
    }
}
