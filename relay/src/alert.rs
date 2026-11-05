//! Telling the operator beyond the log: with `[alerts] command` set, a program
//! run through the shell whenever something needs attention. Only `REAMESH_RELAY_ALERT` and
//! `REAMESH_RELAY_MESSAGE` carry what happened, in the environment, so nothing
//! a client sends can end up in the command line.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tracing::error;

/// A command still running after this long is ended.
const COMMAND_LIMIT: Duration = Duration::from_secs(60);

pub const VAR_KIND: &str = "REAMESH_RELAY_ALERT";
pub const VAR_MESSAGE: &str = "REAMESH_RELAY_MESSAGE";

/// Starts the command; a command that won't start is an error for the caller.
pub fn spawn(command: &str, kind: &str, message: &str) -> std::io::Result<Child> {
    #[cfg(unix)]
    let mut c = {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    };
    #[cfg(not(unix))]
    let mut c = {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(command);
        c
    };
    c.env(VAR_KIND, kind)
        .env(VAR_MESSAGE, message)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// Runs the command in the background; the caller logs what happened.
pub fn fire(command: Option<&str>, kind: &str, message: &str) {
    let Some(command) = command else { return };
    match spawn(command, kind, message) {
        Ok(child) => {
            let kind = kind.to_owned();
            std::thread::spawn(move || reap(child, &kind));
        }
        Err(e) => error!("cannot run the alert command: {e}"),
    }
}

/// Waits for the command so it doesn't stay behind as a zombie, ends it after
/// `COMMAND_LIMIT`, and says when it failed.
fn reap(mut child: Child, kind: &str) {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    error!("the alert command failed on {kind}: {status}");
                }
                return;
            }
            Ok(None) if started.elapsed() > COMMAND_LIMIT => {
                let _ = child.kill();
                let _ = child.wait();
                error!("the alert command took too long on {kind} and was ended");
                return;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(250)),
            Err(e) => {
                error!("the alert command on {kind}: {e}");
                return;
            }
        }
    }
}

/// Hosts leaving is normal, one at a time. Several gone within moments, with
/// none back after a while, looks like the relay can't be reached from outside
/// any more; which a relay can't test on itself (a server rarely reaches its own
/// public address), so this is the sign it has.
#[derive(Debug, Default)]
pub struct HostsWatch {
    /// When several hosts were last online.
    many_at: Option<Instant>,
    gone_since: Option<Instant>,
    fired: bool,
}

/// How quickly the last of several hosts must leave to count as "all at once".
const ALL_AT_ONCE: Duration = Duration::from_secs(30);
/// How long nobody may come back before it counts.
pub const HOSTS_GONE_AFTER: Duration = Duration::from_secs(5 * 60);

impl HostsWatch {
    /// `hosts` online right now; true the one time it should alert.
    pub fn tick(&mut self, hosts: usize, now: Instant) -> bool {
        if hosts >= 2 {
            self.many_at = Some(now);
        }
        if hosts > 0 {
            self.gone_since = None;
            self.fired = false;
            return false;
        }
        if self.gone_since.is_none()
            && self
                .many_at
                .is_some_and(|t| now.duration_since(t) <= ALL_AT_ONCE)
        {
            self.gone_since = Some(now);
        }
        if !self.fired
            && self
                .gone_since
                .is_some_and(|t| now.duration_since(t) >= HOSTS_GONE_AFTER)
        {
            self.fired = true;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_all_gone_alerts_once_and_only_after_the_wait() {
        let t0 = Instant::now();
        let mut w = HostsWatch::default();
        assert!(!w.tick(3, t0));
        assert!(!w.tick(0, t0 + Duration::from_secs(5)));
        let later = t0 + Duration::from_secs(5) + HOSTS_GONE_AFTER;
        assert!(w.tick(0, later));
        assert!(!w.tick(0, later + Duration::from_secs(60)));
        // Someone came back: it can go off again next time.
        assert!(!w.tick(2, later + Duration::from_secs(61)));
        let t1 = later + Duration::from_secs(62);
        assert!(!w.tick(0, t1));
        assert!(w.tick(0, t1 + HOSTS_GONE_AFTER));
    }

    #[test]
    fn one_host_leaving_or_a_slow_drain_is_normal() {
        let t0 = Instant::now();
        let mut w = HostsWatch::default();
        assert!(!w.tick(1, t0));
        assert!(!w.tick(0, t0 + Duration::from_secs(1)));
        assert!(!w.tick(0, t0 + HOSTS_GONE_AFTER * 2));
        // Two hosts, the last one leaves long after the other.
        assert!(!w.tick(2, t0));
        assert!(!w.tick(1, t0 + Duration::from_secs(1)));
        let t1 = t0 + Duration::from_secs(600);
        assert!(!w.tick(1, t1));
        assert!(!w.tick(0, t1 + Duration::from_secs(1)));
        assert!(!w.tick(0, t1 + HOSTS_GONE_AFTER * 2));
    }

    #[test]
    #[cfg(unix)]
    fn the_command_sees_what_happened_in_its_environment() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let cmd = format!(
            "printf '%s|%s' \"${VAR_KIND}\" \"${VAR_MESSAGE}\" > {}",
            out.display()
        );
        // A message with shell syntax in it stays plain data.
        let mut child = spawn(&cmd, "test", "a $(touch nope) b").unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(
            std::fs::read_to_string(out).unwrap(),
            "test|a $(touch nope) b"
        );
    }
}
