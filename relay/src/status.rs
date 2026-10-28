//! `status.json`: the running relay rewrites it every few seconds;
//! `reamesh-relay status` reads it. A file instead of a socket: nothing
//! listens beyond the relay ports, and the data dir's permissions already
//! decide who may read it.

use std::net::IpAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::{Mode, STATUS_FILE};

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
    pub label: String,
    pub peer_id: String,
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

    pub fn render(&self, now: u64) -> String {
        let mut out = String::new();
        let state = if self.is_live(now) {
            format!(
                "running (pid {}, up {})",
                self.pid,
                duration(now.saturating_sub(self.started))
            )
        } else {
            format!(
                "not running (last update {} ago)",
                duration(now.saturating_sub(self.updated))
            )
        };
        let mode = match (self.mode, self.budget_exhausted) {
            (Mode::Full, _) => "full".to_owned(),
            (Mode::Coordination, true) => "coordination (monthly budget used up)".to_owned(),
            (Mode::Coordination, false) => "coordination".to_owned(),
        };
        let budget = if self.budget == 0 {
            "unlimited".to_owned()
        } else {
            format!(
                "{} ({:.0}% used)",
                bytes(self.budget),
                self.sent as f64 * 100.0 / self.budget as f64
            )
        };
        out += &format!("Relay:      {state}\n");
        out += &format!("Version:    {}\n", self.version);
        out += &format!("Peer ID:    {}\n", self.peer_id);
        out += &format!("Mode:       {mode}\n");
        out += &format!(
            "Traffic:    {} sent, {} received in {}\n",
            bytes(self.sent),
            bytes(self.received),
            self.month
        );
        out += &format!("Budget:     {budget}\n");
        out += &format!("Circuits:   {}\n", self.circuits);
        out += &format!("Peers:      {}\n", self.peers);
        out += &format!("Hosts:      {}\n", self.hosts.len());
        for h in &self.hosts {
            out += &format!("  {:<24} {}\n", h.label, h.peer_id);
        }
        out
    }
}

pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn duration(secs: u64) -> String {
    match secs {
        s if s < 120 => format!("{s}s"),
        s if s < 7200 => format!("{}m", s / 60),
        s if s < 172_800 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}
