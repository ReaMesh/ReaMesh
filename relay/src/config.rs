//! The data dir: `identity.key`, `config.toml` (ports, limits, access keys),
//! `traffic.json` (this month's counter) and `status.json` (live state for
//! `reamesh-relay status`).

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

pub const DEFAULT_PORT: u16 = 4001;

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
    pub banned_peers: Vec<String>,
    pub limits: Limits,
    pub keys: Vec<KeyEntry>,
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
            banned_peers: Vec::new(),
            limits: Limits::default(),
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

/// Adds a key to config.toml (comments kept) and returns it.
pub fn add_key(dir: &Path, label: &str) -> anyhow::Result<AccessKey> {
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

# Peer IDs that may not use this relay.
banned_peers = []

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

# Access keys. Each one gives a relay code; hand one to each host (or one for
# everyone). Manage them with `reamesh-relay key add|list|remove`.
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
    fn rejects_bad_config() {
        assert!(Config::parse("tcp_port = 1\nbogus = 2").is_err());
        assert!(Config::parse("[[keys]]\nlabel = \"a\"\nkey = \"abcd\"").is_err());
        assert!(Config::parse("banned_peers = [\"nope\"]").is_err());
        assert!(Config::parse("mode = \"open\"").is_err());
    }
}
