//! The data dir: `identity.key`, `config.toml` (ports, limits, access keys),
//! `traffic.json` (this month's counter), `status.json` (live state for
//! `reamesh-relay status`), `seen.json` (when each key was last used) and
//! `relay.lock` (held while a relay runs on the dir).

use std::collections::HashSet;
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use libp2p::{PeerId, identity::Keypair};
use serde::Deserialize;

use reamesh_relay_proto::AccessKey;

use crate::term::problem;

pub const CONFIG_FILE: &str = "config.toml";
pub const IDENTITY_FILE: &str = "identity.key";
pub const TRAFFIC_FILE: &str = "traffic.json";
pub const STATUS_FILE: &str = "status.json";
pub const SEEN_FILE: &str = "seen.json";
pub const LOCK_FILE: &str = "relay.lock";

pub const DEFAULT_PORT: u16 = 4001;
/// The relay warns (and runs the alert command) at this share of a budget,
/// unless `[alerts] budget_percent` says otherwise.
pub const DEFAULT_ALERT_PERCENT: u8 = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Relay traffic within the limits below.
    Full,
    /// Only enough per circuit for two peers to find a direct path (DCUtR).
    Coordination,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub tcp_port: u16,
    pub quic_port: u16,
    pub listen_addrs: Vec<IpAddr>,
    pub public_addrs: Vec<IpAddr>,
    pub dns_name: Option<String>,
    pub mode: Mode,
    pub log_addresses: bool,
    pub session_list: bool,
    pub banned_peers: Vec<String>,
    /// Who runs this relay. Optional, and ReaMesh only shows them when
    /// something is wrong with the relay.
    pub name: Option<String>,
    pub contact: Option<String>,
    pub limits: Limits,
    pub alerts: Alerts,
    pub update: UpdateConfig,
    pub notice: Option<NoticeEntry>,
    pub keys: Vec<KeyEntry>,
}

/// An announcement for everyone holding a key (`reamesh-relay notice`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoticeEntry {
    pub text: String,
    /// Date and time with offset; without them it counts from now on, until
    /// removed.
    #[serde(default)]
    pub from: Option<toml::value::Datetime>,
    #[serde(default)]
    pub until: Option<toml::value::Datetime>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Alerts {
    /// Run through the shell when something needs the operator's attention,
    /// with `REAMESH_RELAY_ALERT` (what) and `REAMESH_RELAY_MESSAGE` (a
    /// sentence) in its environment.
    pub command: Option<String>,
    /// Share of the monthly budget that sets off a warning, 1 to 100.
    pub budget_percent: u8,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpdateConfig {
    /// Download new versions and restart into them while nothing is being
    /// relayed. Off, the relay only says when one is out.
    pub auto: bool,
    /// `stable`, or `beta` for pre-releases too.
    pub channel: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            auto: true,
            channel: "stable".to_owned(),
        }
    }
}

impl Default for Alerts {
    fn default() -> Self {
        Self {
            command: None,
            budget_percent: DEFAULT_ALERT_PERCENT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max_hosts: usize,
    pub max_circuits: usize,
    pub max_circuits_per_host: usize,
    pub max_circuits_per_ip: usize,
    pub circuit_max_minutes: u64,
    /// 0 = no per-circuit byte limit.
    pub circuit_max_mb: u64,
    /// Bytes sent per calendar month (UTC); 0 = unlimited.
    pub monthly_budget_gb: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyEntry {
    pub label: String,
    pub key: String,
    /// When `key add` (or the first start) created it; keys from before
    /// there was a date have none.
    #[serde(default)]
    pub added: Option<toml::value::Datetime>,
    /// The last day the key works (a date, in the relay's time zone).
    #[serde(default)]
    pub expires: Option<toml::value::Datetime>,
    /// Hosts this key may have online at once.
    #[serde(default)]
    pub max_hosts: Option<usize>,
    /// Data sent per calendar month for this key's hosts and their guests, in
    /// GB. Once used up, its relayed connections only last long enough to go
    /// direct.
    #[serde(default)]
    pub monthly_budget_gb: Option<u64>,
}

impl KeyEntry {
    /// The last day the key works, if it has one.
    pub fn expiry_date(&self) -> Option<jiff::civil::Date> {
        toml_date(self.expires.as_ref()?)
    }

    pub fn is_expired(&self, today: jiff::civil::Date) -> bool {
        self.expiry_date().is_some_and(|d| today > d)
    }

    /// The first second the key no longer works, 0 for never.
    pub fn expires_at(&self) -> u64 {
        self.expiry_date()
            .and_then(|d| d.tomorrow().ok())
            .and_then(|d| d.to_zoned(jiff::tz::TimeZone::system()).ok())
            .map_or(0, |z| z.timestamp().as_second().max(0) as u64)
    }

    pub fn budget_bytes(&self) -> u64 {
        self.monthly_budget_gb
            .unwrap_or(0)
            .saturating_mul(1_000_000_000)
    }
}

/// A TOML date without a time, as a calendar date.
fn toml_date(d: &toml::value::Datetime) -> Option<jiff::civil::Date> {
    if d.time.is_some() {
        return None;
    }
    let date = d.date?;
    jiff::civil::Date::new(date.year as i16, date.month as i8, date.day as i8).ok()
}

/// A TOML date-time with an offset, as Unix seconds.
fn toml_instant(d: &toml::value::Datetime) -> Option<u64> {
    let t: jiff::Timestamp = d.to_string().parse().ok()?;
    u64::try_from(t.as_second()).ok()
}

impl NoticeEntry {
    /// `(from, until)` in Unix seconds, 0 for open-ended.
    pub fn window(&self) -> (u64, u64) {
        let at = |d: &Option<toml::value::Datetime>| d.as_ref().and_then(toml_instant).unwrap_or(0);
        (at(&self.from), at(&self.until))
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tcp_port: DEFAULT_PORT,
            quic_port: DEFAULT_PORT,
            listen_addrs: vec![IpAddr::from([0, 0, 0, 0]), IpAddr::from([0u16; 8])],
            public_addrs: Vec::new(),
            dns_name: None,
            mode: Mode::Full,
            log_addresses: false,
            session_list: false,
            banned_peers: Vec::new(),
            name: None,
            contact: None,
            limits: Limits::default(),
            alerts: Alerts::default(),
            update: UpdateConfig::default(),
            notice: None,
            keys: Vec::new(),
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_hosts: 32,
            max_circuits: 64,
            max_circuits_per_host: 16,
            max_circuits_per_ip: 8,
            circuit_max_minutes: 12 * 60,
            circuit_max_mb: 0,
            monthly_budget_gb: 500,
        }
    }
}

impl Config {
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let path = dir.join(CONFIG_FILE);
        let text = fs::read_to_string(&path).map_err(|e| io_problem("read", &path, &e))?;
        Self::parse(&text).map_err(|e| {
            let what = match e.downcast_ref::<toml::de::Error>() {
                Some(t) => {
                    let line = t
                        .span()
                        .map_or(1, |sp| text[..sp.start].matches('\n').count() + 1);
                    format!(
                        "{}, line {line}: {}",
                        crate::term::path(&path),
                        t.message().trim()
                    )
                }
                None => format!("{}: {e:#}", crate::term::path(&path)),
            };
            problem(
                what,
                "Fix it and start again; the comments in the file explain each setting.",
            )
        })
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let cfg: Config = toml::from_str(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> anyhow::Result<()> {
        let mut seen = HashSet::new();
        for k in &self.keys {
            validate_label(&k.label)?;
            parse_key(&k.key).with_context(|| format!("key {:?}", k.label))?;
            if !seen.insert(k.label.as_str()) {
                bail!("two keys are labeled {:?}", k.label);
            }
            if k.expires.is_some() && k.expiry_date().is_none() {
                bail!("key {:?}: expires must be a date, like 2026-12-24", k.label);
            }
            if k.max_hosts == Some(0) {
                bail!("key {:?}: max_hosts must be at least 1", k.label);
            }
        }
        if let Some(n) = &self.notice {
            validate_notice_text(&n.text)?;
            for (what, value) in [("from", &n.from), ("until", &n.until)] {
                if value.as_ref().is_some_and(|d| toml_instant(d).is_none()) {
                    bail!(
                        "notice: {what} must be a date and time with offset, like 2026-11-11T09:00:00+01:00"
                    );
                }
            }
            let (from, until) = n.window();
            if from != 0 && until != 0 && until <= from {
                bail!("notice: until must be after from");
            }
        }
        for (what, value, max) in [
            ("name", &self.name, reamesh_relay_proto::RELAY_NAME_MAX),
            (
                "contact",
                &self.contact,
                reamesh_relay_proto::RELAY_CONTACT_MAX,
            ),
        ] {
            if let Some(v) = value
                && (v.chars().count() > max || v.chars().any(char::is_control))
            {
                bail!("{what} must be at most {max} printable characters");
            }
        }
        if !matches!(self.update.channel.as_str(), "stable" | "beta") {
            bail!("update.channel must be \"stable\" or \"beta\"");
        }
        if !(1..=100).contains(&self.alerts.budget_percent) {
            bail!("alerts.budget_percent must be between 1 and 100");
        }
        self.banned()?;
        if self.listen_addrs.is_empty() {
            bail!("listen_addrs is empty");
        }
        // The same rule ReaMesh applies to a pasted code, so the relay never
        // hands out one that would be refused.
        if let Some(d) = &self.dns_name
            && !reamesh_relay_proto::valid_dns_name(d)
        {
            bail!("dns_name {d:?} is not a host name");
        }
        if self.limits.circuit_max_minutes == 0
            || self.limits.circuit_max_minutes > u32::MAX as u64 / 60
        {
            bail!(
                "circuit_max_minutes must be between 1 and {}",
                u32::MAX / 60
            );
        }
        Ok(())
    }

    pub fn keys(&self) -> Vec<(String, AccessKey)> {
        self.keys
            .iter()
            .filter_map(|k| Some((k.label.clone(), parse_key(&k.key).ok()?)))
            .collect()
    }

    pub fn key(&self, label: &str) -> Option<AccessKey> {
        self.keys()
            .into_iter()
            .find(|(l, _)| l == label)
            .map(|(_, k)| k)
    }

    pub fn banned(&self) -> anyhow::Result<HashSet<PeerId>> {
        self.banned_peers
            .iter()
            .map(|p| {
                p.parse()
                    .with_context(|| format!("banned_peers: {p:?} is not a peer id"))
            })
            .collect()
    }

    pub fn budget_bytes(&self) -> u64 {
        self.limits.monthly_budget_gb.saturating_mul(1_000_000_000)
    }

    pub fn entry(&self, label: &str) -> Option<&KeyEntry> {
        self.keys.iter().find(|k| k.label == label)
    }

    /// The labels of the keys that work today.
    pub fn active_labels(&self, today: jiff::civil::Date) -> HashSet<String> {
        self.keys
            .iter()
            .filter(|k| !k.is_expired(today))
            .map(|k| k.label.clone())
            .collect()
    }
}

/// Today where the relay runs, the calendar `expires` dates are meant in.
pub fn local_today() -> jiff::civil::Date {
    jiff::Zoned::now().date()
}

pub fn validate_notice_text(text: &str) -> anyhow::Result<()> {
    let max = reamesh_relay_proto::NOTICE_TEXT_MAX;
    if text.trim().is_empty() || text.chars().count() > max || text.chars().any(char::is_control) {
        bail!("a notice must be 1 to {max} printable characters on one line");
    }
    Ok(())
}

pub fn validate_label(label: &str) -> anyhow::Result<()> {
    if label.is_empty() || label.chars().count() > 64 || label.chars().any(char::is_control) {
        bail!("a key label must be 1 to 64 printable characters");
    }
    Ok(())
}

pub fn parse_key(hex_key: &str) -> anyhow::Result<AccessKey> {
    let bytes = hex::decode(hex_key.trim()).context("not hex")?;
    AccessKey::try_from(bytes.as_slice())
        .ok()
        .context("an access key is 32 bytes (64 hex characters)")
}

/// What tells keys apart without showing them.
pub fn fingerprint(key: &AccessKey) -> String {
    blake3::hash(key).to_hex()[..8].to_owned()
}

/// Today in the local time zone, as a TOML date.
fn today() -> toml_edit::Datetime {
    let d = jiff::Zoned::now().date();
    toml_edit::Datetime {
        date: Some(toml_edit::Date {
            year: d.year() as u16,
            month: d.month() as u8,
            day: d.day() as u8,
        }),
        time: None,
        offset: None,
    }
}

/// A file the relay can't read or write, said plainly.
pub fn io_problem(verb: &str, path: &Path, e: &std::io::Error) -> anyhow::Error {
    let dir = path.parent().unwrap_or(path);
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => problem(
            format!("No permission to {verb} {}.", crate::term::path(path)),
            format!(
                "Run it as the user that owns {}, or pick another data dir with --data.",
                crate::term::path(dir)
            ),
        ),
        _ => problem(
            format!("Cannot {verb} {}: {e}.", crate::term::path(path)),
            "",
        ),
    }
}

/// Claims the data dir for a running relay. The lock lives as long as the
/// returned file, and the OS lets go of it when the process ends, crashed
/// or not, so a restart right after a crash is never refused.
pub fn lock_data_dir(dir: &Path) -> anyhow::Result<fs::File> {
    let path = dir.join(LOCK_FILE);
    let file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| io_problem("open", &path, &e))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(fs::TryLockError::WouldBlock) => {
            let since = crate::status::Status::read(dir)
                .filter(|s| s.running)
                .map(|s| format!(" (since {})", crate::term::local_since(s.started)))
                .unwrap_or_default();
            Err(problem(
                format!("A relay already runs with this data dir{since}."),
                "See it with: reamesh-relay status\nA second relay needs its own data dir (--data) and ports.",
            ))
        }
        Err(fs::TryLockError::Error(e)) => Err(io_problem("lock", &path, &e)),
    }
}

pub fn new_key() -> AccessKey {
    let mut k = [0u8; 32];
    getrandom::fill(&mut k).expect("OS random number generator");
    k
}

/// The data dir: `--data`, else `REAMESH_RELAY_DATA` (both via clap), else a
/// per-user default.
pub fn default_data_dir() -> PathBuf {
    if cfg!(windows)
        && let Some(appdata) = std::env::var_os("APPDATA")
    {
        return PathBuf::from(appdata).join("reamesh-relay");
    }
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".reamesh-relay"),
        None => PathBuf::from("reamesh-relay-data"),
    }
}

/// Creates the data dir on first start: identity, one access key labeled
/// "default" and a commented config. Returns whether anything was created.
pub fn init(dir: &Path) -> anyhow::Result<bool> {
    fs::create_dir_all(dir).map_err(|e| io_problem("create", dir, &e))?;
    restrict_dir(dir);
    let mut created = false;
    let id_path = dir.join(IDENTITY_FILE);
    if !id_path.exists() {
        let key = Keypair::generate_ed25519();
        write_private(&id_path, &key.to_protobuf_encoding()?)?;
        created = true;
    }
    let cfg_path = dir.join(CONFIG_FILE);
    if !cfg_path.exists() {
        let text = template(&hex::encode(new_key()), &today().to_string());
        write_private(&cfg_path, text.as_bytes())?;
        created = true;
    }
    Ok(created)
}

pub fn load_identity(dir: &Path) -> anyhow::Result<Keypair> {
    let path = dir.join(IDENTITY_FILE);
    let bytes = fs::read(&path).map_err(|e| io_problem("read", &path, &e))?;
    Keypair::from_protobuf_encoding(&bytes).map_err(|_| {
        problem(
            format!("{} is damaged.", crate::term::path(&path)),
            "Restore it from a backup. Deleting it gives the relay a new identity, and every relay code has to be handed out again.",
        )
    })
}

fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|e| io_problem("write", path, &e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
        // `sudo reamesh-relay key add` must not leave a root-owned config the
        // service user can't read: keep the owner of the file it replaces.
        let owner = fs::metadata(path).or_else(|_| fs::metadata(path.parent().unwrap_or(path)));
        if let Ok(m) = owner {
            let _ = std::os::unix::fs::chown(&tmp, Some(m.uid()), Some(m.gid()));
        }
    }
    fs::rename(&tmp, path).map_err(|e| io_problem("write", path, &e))
}

fn restrict_dir(_dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(_dir, fs::Permissions::from_mode(0o700));
    }
}

/// A date like `2026-12-24` or a span like `30d`, `6w`, as the last day a key
/// works. Spans count from today.
pub fn parse_expiry(text: &str, today: jiff::civil::Date) -> anyhow::Result<jiff::civil::Date> {
    let text = text.trim();
    if let Ok(d) = text.parse::<jiff::civil::Date>() {
        return Ok(d);
    }
    let bad = || {
        problem(
            format!("{text:?} is neither a date nor a span."),
            "Use a date like 2026-12-24, or a span like 30d or 6w.",
        )
    };
    let split = text.len().saturating_sub(1);
    if !text.is_char_boundary(split) {
        return Err(bad());
    }
    let (n, unit) = text.split_at(split);
    let n: i64 = n.parse().map_err(|_| bad())?;
    let days = match unit {
        "d" => n,
        "w" => n.saturating_mul(7),
        _ => return Err(bad()),
    };
    if !(1..=36_500).contains(&days) {
        return Err(bad());
    }
    today
        .checked_add(jiff::Span::new().days(days))
        .map_err(|_| bad())
}

fn toml_date_value(d: jiff::civil::Date) -> toml_edit::Datetime {
    toml_edit::Datetime {
        date: Some(toml_edit::Date {
            year: d.year() as u16,
            month: d.month() as u8,
            day: d.day() as u8,
        }),
        time: None,
        offset: None,
    }
}

/// Sets or clears a key's last day.
pub fn set_key_expiry(
    dir: &Path,
    label: &str,
    expires: Option<jiff::civil::Date>,
) -> anyhow::Result<()> {
    let cfg = Config::load(dir)?;
    if !cfg.keys.iter().any(|k| k.label == label) {
        return Err(no_such_key(label));
    }
    edit_config(dir, |doc| {
        let keys = doc
            .get_mut("keys")
            .and_then(|k| k.as_array_of_tables_mut())
            .context("`keys` in config.toml is not a [[keys]] list")?;
        for t in keys.iter_mut() {
            if t.get("label").and_then(|l| l.as_str()) == Some(label) {
                match expires {
                    Some(d) => t["expires"] = toml_edit::value(toml_date_value(d)),
                    None => {
                        t.remove("expires");
                    }
                }
            }
        }
        Ok(())
    })
}

/// A time the operator typed: `2026-11-11 09:00`, `2026-11-11T09:00` or a full
/// RFC 3339 time. Without an offset it means the relay's time zone.
pub fn parse_notice_time(text: &str) -> anyhow::Result<jiff::Timestamp> {
    let text = text.trim();
    if let Ok(t) = text.parse::<jiff::Timestamp>() {
        return Ok(t);
    }
    text.parse::<jiff::civil::DateTime>()
        .ok()
        .and_then(|d| d.to_zoned(jiff::tz::TimeZone::system()).ok())
        .map(|z| z.timestamp())
        .ok_or_else(|| {
            problem(
                format!("{text:?} is not a date and time."),
                "Use the form 2026-11-11 09:00.",
            )
        })
}

/// The form config.toml keeps a notice's times in: to the second, with this
/// machine's offset, so the file reads as what the operator typed.
fn notice_time_value(t: jiff::Timestamp) -> anyhow::Result<toml_edit::Datetime> {
    let z = t.to_zoned(jiff::tz::TimeZone::system());
    z.strftime("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
        .parse()
        .context("formatting a time")
}

/// Sets the notice (replacing any) or, with no text, removes it.
pub fn set_notice(
    dir: &Path,
    notice: Option<(&str, Option<jiff::Timestamp>, Option<jiff::Timestamp>)>,
) -> anyhow::Result<()> {
    edit_config(dir, |doc| {
        doc.remove("notice");
        let Some((text, from, until)) = notice else {
            return Ok(());
        };
        let mut t = toml_edit::Table::new();
        t["text"] = toml_edit::value(text);
        if let Some(from) = from {
            t["from"] = toml_edit::value(notice_time_value(from)?);
        }
        if let Some(until) = until {
            t["until"] = toml_edit::value(notice_time_value(until)?);
        }
        doc.insert("notice", toml_edit::Item::Table(t));
        Ok(())
    })
}

/// Adds a key to config.toml (comments kept) and returns it.
pub fn add_key(dir: &Path, label: &str) -> anyhow::Result<AccessKey> {
    add_key_until(dir, label, None)
}

/// `add_key` with a last day.
pub fn add_key_until(
    dir: &Path,
    label: &str,
    expires: Option<jiff::civil::Date>,
) -> anyhow::Result<AccessKey> {
    validate_label(label)?;
    let cfg = Config::load(dir)?;
    if cfg.keys.iter().any(|k| k.label == label) {
        return Err(problem(
            format!("A key named {label} already exists."),
            "Pick another name, or see the keys with: reamesh-relay key list",
        ));
    }
    let key = new_key();
    edit_config(dir, |doc| {
        let keys = doc
            .entry("keys")
            .or_insert(toml_edit::Item::ArrayOfTables(Default::default()))
            .as_array_of_tables_mut()
            .context("`keys` in config.toml is not a [[keys]] list")?;
        let mut t = toml_edit::Table::new();
        t["label"] = toml_edit::value(label);
        t["key"] = toml_edit::value(hex::encode(key));
        t["added"] = toml_edit::value(today());
        if let Some(d) = expires {
            t["expires"] = toml_edit::value(toml_date_value(d));
        }
        keys.push(t);
        Ok(())
    })?;
    Ok(key)
}

pub fn remove_key(dir: &Path, label: &str) -> anyhow::Result<()> {
    let cfg = Config::load(dir)?;
    if !cfg.keys.iter().any(|k| k.label == label) {
        return Err(no_such_key(label));
    }
    edit_config(dir, |doc| {
        let keys = doc
            .get_mut("keys")
            .and_then(|k| k.as_array_of_tables_mut())
            .context("`keys` in config.toml is not a [[keys]] list")?;
        let Some(i) = keys
            .iter()
            .position(|t| t.get("label").and_then(|l| l.as_str()) == Some(label))
        else {
            return Ok(());
        };
        let removed = keys.remove(i);
        // Comments above the removed key (the template's explanation sits
        // above the first one) move to the key that takes its place.
        let comment = removed
            .decor()
            .prefix()
            .and_then(|p| p.as_str())
            .unwrap_or("")
            .to_owned();
        if let Some(next) = keys.get_mut(i)
            && !comment.trim().is_empty()
        {
            let own = next
                .decor()
                .prefix()
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_owned();
            next.decor_mut()
                .set_prefix(format!("{comment}{}", own.trim_start_matches('\n')));
        }
        Ok(())
    })
}

/// `reamesh-relay list on|off`. A config from before the setting gets it with
/// its explanation, like the template has it.
pub fn set_session_list(dir: &Path, on: bool) -> anyhow::Result<()> {
    edit_config(dir, |doc| {
        let new = doc.get("session_list").is_none();
        doc["session_list"] = toml_edit::value(on);
        if new
            && let Some(mut key) = doc.as_table_mut().key_mut("session_list")
        {
            key.leaf_decor_mut().set_prefix(
                "\n# Everyone with a key sees the sessions hosted through this relay, and can join\n\
                 # open ones with one click. false: the relay keeps no list and sees no session\n\
                 # names. Switch it with `reamesh-relay list on|off`.\n",
            );
        }
        Ok(())
    })
}

fn edit_config(
    dir: &Path,
    f: impl FnOnce(&mut toml_edit::DocumentMut) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let path = dir.join(CONFIG_FILE);
    let text = fs::read_to_string(&path)?;
    let mut doc: toml_edit::DocumentMut = text.parse().context("config.toml is not valid TOML")?;
    f(&mut doc)?;
    let out = doc.to_string();
    Config::parse(&out).context("the edited config would not load")?;
    write_private(&path, out.as_bytes())
}

/// The config written on first start: every setting, commented.
pub fn no_such_key(label: &str) -> anyhow::Error {
    problem(
        format!("No key named {label}."),
        "See the keys with: reamesh-relay key list",
    )
}

pub fn template(default_key: &str, added: &str) -> String {
    format!(
        r#"# ReaMesh relay configuration.
#
# The relay helps ReaMesh hosts and guests reach each other across routers and
# firewalls: it introduces them so they can connect directly, and passes their
# traffic along when a direct path can't be found. You run it for your own
# circle; only hosts holding one of the access keys below can use it.
#
# Changes take effect without a restart (the relay watches this file; on
# Linux/macOS `kill -HUP` reloads it at once). Changing a port, the mode or a
# per-circuit limit briefly reconnects everyone.

# Ports, TCP and UDP (QUIC). Open both in your firewall. 443 gets through most
# restrictive networks (hotel, office) if nothing else on the server uses it;
# ports below 1024 need root or CAP_NET_BIND_SERVICE.
tcp_port = {DEFAULT_PORT}
quic_port = {DEFAULT_PORT}

# Local addresses to listen on ("::" is IPv6; drop it on a host without IPv6).
listen_addrs = ["0.0.0.0", "::"]

# The public addresses put into relay codes. Left empty, the relay uses the
# public IPs of its network interfaces. Behind NAT (Oracle Cloud, AWS, a home
# router with port forwarding) set the public IP here, or a DNS name below.
# Addresses listed here are used as they are, even private ones.
# public_addrs = ["203.0.113.7"]

# A DNS name for the relay (for a changing home IP, use a dynamic DNS name).
# dns_name = "relay.example.org"

# "full": relay traffic within the limits below.
# "coordination": only help peers find a direct path; no bulk traffic. The
# relay also switches to this by itself once the monthly budget is used up.
mode = "full"

# Client IP addresses are never logged unless this is true.
log_addresses = false

# Everyone with a key sees the sessions hosted through this relay, and can join
# open ones with one click. false: the relay keeps no list and sees no session
# names. Switch it with `reamesh-relay list on|off`.
session_list = false

# Peer IDs that may not use this relay.
banned_peers = []

# Who runs this relay. ReaMesh only shows these when something is wrong with
# the relay, so hosts know whom to ask. Both optional.
# name = "Studio relay"
# contact = "lena@example.org"

[limits]
# Hosts that may hold a reservation at the same time.
max_hosts = 32
# Relayed connections at the same time: in total, per host, per guest IP.
max_circuits = 64
max_circuits_per_host = 16
max_circuits_per_ip = 8
# A relayed connection ends after this long or this many MB (0 = no MB limit);
# ReaMesh then reconnects it.
circuit_max_minutes = 720
circuit_max_mb = 0
# Data the relay may send per calendar month (UTC), in GB (0 = unlimited).
# Once used up it switches to coordination mode until the month ends.
monthly_budget_gb = 500

[alerts]
# Runs through the shell when something needs your attention: the budget nearly
# or fully used up, a key's budget used up, a key that expired, UDP that looks
# blocked, or every host gone at once with none coming back. It gets
# REAMESH_RELAY_ALERT (what happened) and REAMESH_RELAY_MESSAGE (a sentence) in
# its environment. Try it with `reamesh-relay alert test`.
# command = "curl -s -d \"$REAMESH_RELAY_MESSAGE\" https://ntfy.sh/my-relay"
# Share of the monthly budget that sets off a warning.
budget_percent = {DEFAULT_ALERT_PERCENT}

[update]
# Download new versions (signed by the ReaMesh release key) and restart into them
# at a moment nothing is being relayed. Hosts reconnect by themselves. With
# false the relay only says when one is out; `reamesh-relay update` then gets it.
auto = true
# "stable", or "beta" for pre-releases too.
channel = "stable"

# Access keys. Each one gives a relay code; hand one to each host (or one for
# everyone). Manage them with `reamesh-relay key add|list|expires|remove`.
# A key can also have, each optional:
#   expires = 2026-12-24      the last day it works
#   max_hosts = 2             hosts it may have online at once
#   monthly_budget_gb = 50    its own share of the traffic; once used up, its
#                             relayed connections only last long enough to go
#                             direct
[[keys]]
label = "default"
key = "{default_key}"
added = {added}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_to_defaults() {
        let key = hex::encode([7u8; 32]);
        let cfg = Config::parse(&template(&key, "2026-10-28")).unwrap();
        let want = Config {
            keys: vec![KeyEntry {
                label: "default".into(),
                key,
                added: Some("2026-10-28".parse().unwrap()),
                expires: None,
                max_hosts: None,
                monthly_budget_gb: None,
            }],
            ..Config::default()
        };
        assert_eq!(cfg, want);
    }

    #[test]
    fn key_add_and_remove_keep_comments() {
        let dir = tempfile::tempdir().unwrap();
        assert!(init(dir.path()).unwrap());
        assert!(!init(dir.path()).unwrap());
        let k = add_key(dir.path(), "Lena").unwrap();
        assert!(add_key(dir.path(), "Lena").is_err());
        let cfg = Config::load(dir.path()).unwrap();
        assert_eq!(cfg.key("Lena"), Some(k));
        assert_eq!(cfg.keys().len(), 2);
        remove_key(dir.path(), "default").unwrap();
        assert!(remove_key(dir.path(), "default").is_err());
        let text = fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap();
        assert!(text.contains("# Access keys."));
        let cfg = Config::load(dir.path()).unwrap();
        assert_eq!(
            cfg.keys
                .iter()
                .map(|k| k.label.as_str())
                .collect::<Vec<_>>(),
            ["Lena"]
        );
    }

    #[test]
    fn session_list_is_off_until_switched_and_keeps_the_file_intact() {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path()).unwrap();
        assert!(!Config::load(dir.path()).unwrap().session_list);
        set_session_list(dir.path(), true).unwrap();
        assert!(Config::load(dir.path()).unwrap().session_list);
        let text = fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap();
        assert!(text.contains("session_list = true"));
        assert!(text.contains("# Access keys."));
        set_session_list(dir.path(), false).unwrap();
        assert!(!Config::load(dir.path()).unwrap().session_list);
    }

    #[test]
    fn a_config_from_before_the_list_loads_and_can_turn_it_on() {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path()).unwrap();
        let path = dir.path().join(CONFIG_FILE);
        // The setting and its comment as an older relay wrote the file.
        let old: String = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .filter(|l| {
                !l.starts_with("session_list")
                    && !l.starts_with("# Everyone with a key")
                    && !l.starts_with("# open ones with")
                    && !l.starts_with("# names. Switch it")
            })
            .map(|l| format!("{l}\n"))
            .collect();
        fs::write(&path, old).unwrap();
        assert!(!Config::load(dir.path()).unwrap().session_list);
        set_session_list(dir.path(), true).unwrap();
        let cfg = Config::load(dir.path()).unwrap();
        assert!(cfg.session_list);
        assert_eq!(cfg.limits, Limits::default());
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# Everyone with a key sees"));
        assert!(text.find("session_list").unwrap() < text.find("[limits]").unwrap());
    }

    #[test]
    fn config_errors_name_the_line() {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path()).unwrap();
        let path = dir.path().join(CONFIG_FILE);
        let text = fs::read_to_string(&path)
            .unwrap()
            .replace("tcp_port = 4001", "tcp_port = \"4001\"");
        let line = text
            .lines()
            .position(|l| l.starts_with("tcp_port"))
            .unwrap()
            + 1;
        fs::write(&path, text).unwrap();
        let e = Config::load(dir.path()).unwrap_err();
        let p = e.downcast_ref::<crate::term::Problem>().unwrap();
        assert!(p.what.contains(&format!("line {line}: ")), "{}", p.what);
    }

    #[test]
    fn expiry_counts_the_last_day_as_valid() {
        let cfg = Config::parse(
            "[[keys]]\nlabel = \"a\"\nkey = \"0000000000000000000000000000000000000000000000000000000000000000\"\nexpires = 2026-12-24\n",
        )
        .unwrap();
        let k = &cfg.keys[0];
        let day: jiff::civil::Date = "2026-12-24".parse().unwrap();
        assert!(!k.is_expired(day));
        assert!(k.is_expired(day.tomorrow().unwrap()));
        assert!(k.expires_at() > 0);
    }

    #[test]
    fn expiry_parses_dates_and_spans() {
        let today: jiff::civil::Date = "2026-11-05".parse().unwrap();
        let d = |s: &str| parse_expiry(s, today).unwrap().to_string();
        assert_eq!(d("2026-12-24"), "2026-12-24");
        assert_eq!(d("30d"), "2026-12-05");
        assert_eq!(d("2w"), "2026-11-19");
        for bad in ["soon", "0d", "-3d", "5y", "", "ä"] {
            assert!(parse_expiry(bad, today).is_err(), "{bad}");
        }
    }

    #[test]
    fn key_and_notice_commands_edit_the_file() {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path()).unwrap();
        let day: jiff::civil::Date = "2026-12-24".parse().unwrap();
        add_key_until(dir.path(), "gast", Some(day)).unwrap();
        assert_eq!(
            Config::load(dir.path())
                .unwrap()
                .entry("gast")
                .unwrap()
                .expiry_date(),
            Some(day)
        );
        set_key_expiry(dir.path(), "gast", None).unwrap();
        assert_eq!(
            Config::load(dir.path())
                .unwrap()
                .entry("gast")
                .unwrap()
                .expires,
            None
        );
        assert!(set_key_expiry(dir.path(), "nobody", None).is_err());

        let from = parse_notice_time("2026-11-11T09:00:00+01:00").unwrap();
        let until = parse_notice_time("2026-11-11 14:00").unwrap();
        set_notice(dir.path(), Some(("Serverumzug", Some(from), Some(until)))).unwrap();
        let cfg = Config::load(dir.path()).unwrap();
        let n = cfg.notice.unwrap();
        assert_eq!(n.text, "Serverumzug");
        assert_eq!(
            n.window(),
            (from.as_second() as u64, until.as_second() as u64)
        );
        // The key tables stay intact behind the new table.
        assert_eq!(cfg.keys.len(), 2);
        set_notice(dir.path(), None).unwrap();
        assert_eq!(Config::load(dir.path()).unwrap().notice, None);
        // Times the wrong way round don't get in.
        assert!(set_notice(dir.path(), Some(("x", Some(until), Some(from)))).is_err());
    }

    #[test]
    fn rejects_bad_config() {
        assert!(Config::parse("[notice]\ntext = \"\"").is_err());
        assert!(Config::parse("[notice]\ntext = \"x\"\nfrom = 2026-11-11").is_err());
        assert!(Config::parse("name = \"a\\nb\"").is_err());
        assert!(Config::parse("[alerts]\nbudget_percent = 0").is_err());
        let k = "key = \"0000000000000000000000000000000000000000000000000000000000000000\"";
        assert!(
            Config::parse(&format!(
                "[[keys]]\nlabel = \"a\"\n{k}\nexpires = 2026-12-24T10:00:00Z"
            ))
            .is_err()
        );
        assert!(Config::parse(&format!("[[keys]]\nlabel = \"a\"\n{k}\nmax_hosts = 0")).is_err());
        assert!(Config::parse("tcp_port = 1\nbogus = 2").is_err());
        assert!(Config::parse("[[keys]]\nlabel = \"a\"\nkey = \"abcd\"").is_err());
        assert!(Config::parse("banned_peers = [\"nope\"]").is_err());
        assert!(Config::parse("mode = \"open\"").is_err());
    }
}
