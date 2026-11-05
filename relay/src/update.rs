//! Keeping the relay up to date: every few hours it reads the release channel's
//! manifest, downloads a newer build into `<data dir>/update/`, checks it and
//! restarts into it once nothing is being relayed.
//!
//! The copy lives in the data dir because that is the one place a relay may
//! write: the service runs as a throwaway user and the installed binary (or the
//! container image) belongs to root. `run` starts from the installed binary and
//! moves on to the copy when that is newer (`start_staged`), so the installed
//! binary stays the fallback and a newer install simply wins.
//!
//! What makes that safe:
//! - Every build is signed with the release key (the same one that signs the
//!   app's updates) and the signature is checked after the download and again
//!   before every start.
//! - The copy has to run and report exactly the announced version before it is
//!   kept, which also rules out a signed older build or another platform's.
//! - A build that doesn't get as far as listening is dropped on the next start
//!   and that version is never taken again (`TRYING_FILE`, `BAD_FILE`).
//! - Only `run` ever starts the copy. The other commands, which an admin may
//!   run as root, never execute a file the service user can write.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use serde::Deserialize;

/// The public half of the release key: `plugins.updater.pubkey` in the app's
/// tauri.conf.json (a test keeps the two equal).
const RELEASE_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEE5QTA5RTlBMTJEQjhDN0EKUldSNmpOc1NtcDZncVhaM3RoUzF1TUNDZ2JlSmw5dWVjR3R2U1lSbEEyd0pYOG5tN0pwOUNDQlkK";
/// The channel manifests (`beta.json`, `stable.json`), the ones the app reads.
const CHANNEL_BASE: &str = "https://github.com/ReaMesh/ReaMesh/releases/download/updater/";

const UPDATE_DIR: &str = "update";
const BINARY_FILE: &str = "reamesh-relay";
const SIGNATURE_FILE: &str = "reamesh-relay.sig";
const VERSION_FILE: &str = "version";
/// The version being started, until it reaches `ready`.
const TRYING_FILE: &str = "trying";
/// A version that failed to start; it is not downloaded again.
const BAD_FILE: &str = "bad";

const MANIFEST_MAX: u64 = 1024 * 1024;
const BINARY_MAX: u64 = 256 * 1024 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(600);
const SMOKE_TIMEOUT: Duration = Duration::from_secs(10);

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    /// One build per platform, `linux-x86_64` and `linux-aarch64`.
    #[serde(default)]
    relay: std::collections::HashMap<String, Build>,
}

#[derive(Debug, Clone, Deserialize)]
struct Build {
    url: String,
    signature: String,
}

/// Semver order: the numbers first, then a pre-release sorts before its release
/// and tags compare part by part. Mirrors `compareVersions` in the app.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn split(v: &str) -> (Vec<u64>, &str) {
        let v = v.trim().trim_start_matches('v');
        let v = v.split('+').next().unwrap_or(v);
        let (core, pre) = v.split_once('-').unwrap_or((v, ""));
        (
            core.split('.').map(|n| n.parse().unwrap_or(0)).collect(),
            pre,
        )
    }
    let (x, y) = (split(a), split(b));
    for i in 0..x.0.len().max(y.0.len()) {
        match x.0.get(i).unwrap_or(&0).cmp(y.0.get(i).unwrap_or(&0)) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    match (x.1.is_empty(), y.1.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => {
            let (mut p, mut q) = (x.1.split('.'), y.1.split('.'));
            loop {
                match (p.next(), q.next()) {
                    (None, None) => return Ordering::Equal,
                    (None, Some(_)) => return Ordering::Less,
                    (Some(_), None) => return Ordering::Greater,
                    (Some(a), Some(b)) => {
                        let order = match (a.parse::<u64>(), b.parse::<u64>()) {
                            (Ok(a), Ok(b)) => a.cmp(&b),
                            (Ok(_), Err(_)) => Ordering::Less,
                            (Err(_), Ok(_)) => Ordering::Greater,
                            (Err(_), Err(_)) => a.cmp(b),
                        };
                        if order != Ordering::Equal {
                            return order;
                        }
                    }
                }
            }
        }
    }
}

pub fn is_newer(candidate: &str, than: &str) -> bool {
    compare_versions(candidate, than) == std::cmp::Ordering::Greater
}

/// Debug builds accept another key and manifest, for the end-to-end test.
/// Release builds always use the real ones, whatever the environment says.
fn debug_env(name: &str) -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var(name).ok()
    } else {
        None
    }
}

/// How often a running relay looks for a finished download.
pub fn restart_poll(normal: Duration) -> Duration {
    debug_env("REAMESH_RELAY_TEST_POLL_MS")
        .and_then(|ms| ms.parse().ok())
        .map_or(normal, Duration::from_millis)
}

fn release_pubkey() -> String {
    debug_env("REAMESH_RELAY_TEST_PUBKEY").unwrap_or_else(|| RELEASE_PUBKEY.to_owned())
}

/// The manifest's name for this machine's build; `None` where no build is
/// published, which simply means no updates.
fn platform() -> Option<String> {
    if let Some(p) = debug_env("REAMESH_RELAY_TEST_PLATFORM") {
        return Some(p);
    }
    if !cfg!(target_os = "linux") {
        return None;
    }
    match std::env::consts::ARCH {
        "x86_64" => Some("linux-x86_64".to_owned()),
        "aarch64" => Some("linux-aarch64".to_owned()),
        _ => None,
    }
}

/// `signature` is what `tauri signer sign` writes: a base64-wrapped minisign
/// signature file, and `pubkey` the base64-wrapped key file.
fn verify(pubkey: &str, data: &[u8], signature: &str) -> anyhow::Result<()> {
    use base64::Engine as _;
    let text = |b64: &str| -> anyhow::Result<String> {
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim())?;
        Ok(String::from_utf8(bytes)?)
    };
    let key = minisign_verify::PublicKey::decode(&text(pubkey)?)
        .map_err(|e| anyhow::anyhow!("bad public key: {e}"))?;
    let sig = minisign_verify::Signature::decode(&text(signature)?)
        .map_err(|e| anyhow::anyhow!("bad signature: {e}"))?;
    key.verify(data, &sig, true)
        .map_err(|e| anyhow::anyhow!("the signature does not match: {e}"))
}

fn dir_of(data: &Path) -> PathBuf {
    data.join(UPDATE_DIR)
}

fn read_trimmed(path: &Path) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

/// The version of the copy in the data dir, if any.
pub fn staged_version(data: &Path) -> Option<String> {
    let dir = dir_of(data);
    dir.join(BINARY_FILE)
        .is_file()
        .then(|| read_trimmed(&dir.join(VERSION_FILE)))
        .flatten()
}

/// A copy newer than what runs, waiting for a restart.
pub fn pending_version(data: &Path) -> Option<String> {
    staged_version(data).filter(|v| is_newer(v, VERSION))
}

fn discard_staged(data: &Path) {
    let dir = dir_of(data);
    for f in [BINARY_FILE, SIGNATURE_FILE, VERSION_FILE, TRYING_FILE] {
        let _ = fs::remove_file(dir.join(f));
    }
}

/// `sudo reamesh-relay update` must not leave files the service user can't read.
fn keep_dir_owner(_path: &Path, _dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(m) = fs::metadata(_dir) {
            let _ = std::os::unix::fs::chown(_path, Some(m.uid()), Some(m.gid()));
        }
    }
}

fn make_private_executable(_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(_path, fs::Permissions::from_mode(0o700));
    }
}

/// Runs `binary --version` and looks for exactly `version`.
fn reports_version(binary: &Path, version: &str) -> anyhow::Result<()> {
    let mut child = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("it does not start here (a data dir mounted noexec?)")?;
    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if started.elapsed() > SMOKE_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("it did not answer");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = child.wait_with_output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || !text.split_whitespace().any(|w| w == version) {
        bail!("it does not report version {version}");
    }
    Ok(())
}

/// Checks a downloaded build and keeps it as the copy to start.
pub fn stage(
    data: &Path,
    pubkey: &str,
    version: &str,
    binary: &[u8],
    signature: &str,
) -> anyhow::Result<()> {
    verify(pubkey, binary, signature)?;
    let dir = dir_of(data);
    fs::create_dir_all(&dir)?;
    keep_dir_owner(&dir, data);
    let tmp = dir.join(format!("{BINARY_FILE}.download"));
    let _ = fs::remove_file(&tmp);
    fs::write(&tmp, binary)?;
    make_private_executable(&tmp);
    keep_dir_owner(&tmp, &dir);
    if let Err(e) = reports_version(&tmp, version) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    // In this order: a crash in between leaves a version file that names a
    // binary it doesn't match, and the signature check refuses that pair.
    discard_staged(data);
    fs::write(dir.join(SIGNATURE_FILE), signature)?;
    fs::rename(&tmp, dir.join(BINARY_FILE))?;
    fs::write(dir.join(VERSION_FILE), version)?;
    for f in [SIGNATURE_FILE, VERSION_FILE] {
        keep_dir_owner(&dir.join(f), &dir);
    }
    Ok(())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .https_only(!cfg!(debug_assertions))
        .user_agent(concat!("reamesh-relay/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn fetch(url: &str, max: u64) -> anyhow::Result<Vec<u8>> {
    let mut response = agent().get(url).call()?;
    let body = response.body_mut().with_config().limit(max).read_to_vec()?;
    Ok(body)
}

fn manifest_url(channel: &str) -> String {
    debug_env("REAMESH_RELAY_TEST_MANIFEST")
        .unwrap_or_else(|| format!("{CHANNEL_BASE}{channel}.json"))
}

/// What a look at the channel found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checked {
    /// No newer build for this machine.
    UpToDate,
    Available(String),
    /// Downloaded and checked; starts with the next restart.
    Staged(String),
}

fn pick(manifest: &Manifest, platform: &str, current: &str, bad: Option<&str>) -> Option<Build> {
    if !is_newer(&manifest.version, current) || bad == Some(manifest.version.as_str()) {
        return None;
    }
    manifest.relay.get(platform).cloned()
}

/// Reads the channel and, with `install`, downloads what is new.
pub fn check(data: &Path, channel: &str, install: bool) -> anyhow::Result<Checked> {
    let Some(platform) = platform() else {
        return Ok(Checked::UpToDate);
    };
    let bytes = fetch(&manifest_url(channel), MANIFEST_MAX).context("reading the channel")?;
    let manifest: Manifest = serde_json::from_slice(&bytes).context("reading the channel")?;
    let bad = read_trimmed(&dir_of(data).join(BAD_FILE));
    let Some(build) = pick(&manifest, &platform, VERSION, bad.as_deref()) else {
        return Ok(Checked::UpToDate);
    };
    if staged_version(data).as_deref() == Some(manifest.version.as_str()) {
        return Ok(Checked::Staged(manifest.version));
    }
    if !install {
        return Ok(Checked::Available(manifest.version));
    }
    let binary = fetch(&build.url, BINARY_MAX).context("downloading it")?;
    stage(
        data,
        &release_pubkey(),
        &manifest.version,
        &binary,
        &build.signature,
    )
    .with_context(|| format!("version {}", manifest.version))?;
    Ok(Checked::Staged(manifest.version))
}

/// The copy to start now, if there is a good one. A copy whose last start never
/// reached `ready` is dropped here, and that version is never taken again.
fn staged_to_start(data: &Path, pubkey: &str) -> Option<PathBuf> {
    let dir = dir_of(data);
    let version = staged_version(data)?;
    if !is_newer(&version, VERSION) {
        // What runs is as new. Older leftovers go; the same version stays,
        // since it may be the copy that is running right now.
        if is_newer(VERSION, &version) {
            discard_staged(data);
        }
        return None;
    }
    if read_trimmed(&dir.join(TRYING_FILE)).as_deref() == Some(version.as_str()) {
        tracing::warn!(
            kind = "update",
            "version {version} did not start, staying on {VERSION}"
        );
        let _ = fs::write(dir.join(BAD_FILE), &version);
        discard_staged(data);
        return None;
    }
    let binary = dir.join(BINARY_FILE);
    let checked = fs::read(&binary)
        .map_err(anyhow::Error::from)
        .and_then(|bytes| {
            let sig = fs::read_to_string(dir.join(SIGNATURE_FILE))?;
            verify(pubkey, &bytes, &sig)
        });
    if let Err(e) = checked {
        tracing::warn!(
            kind = "update",
            "dropping the downloaded version {version}: {e:#}"
        );
        discard_staged(data);
        return None;
    }
    let _ = fs::write(dir.join(TRYING_FILE), &version);
    Some(binary)
}

/// Replaces this process with the newer copy, if there is one. Returns when
/// there isn't (or it can't be started).
pub fn start_staged(data: &Path) {
    #[cfg(unix)]
    if let Some(binary) = staged_to_start(data, &release_pubkey()) {
        use std::os::unix::process::CommandExt;
        let e = Command::new(&binary)
            .args(std::env::args_os().skip(1))
            .exec();
        tracing::error!("cannot start {}: {e}", binary.display());
        let _ = fs::remove_file(dir_of(data).join(TRYING_FILE));
    }
    #[cfg(not(unix))]
    let _ = data;
}

/// This build is listening: it is good.
pub fn confirm_started(data: &Path) {
    let _ = fs::remove_file(dir_of(data).join(TRYING_FILE));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::{Equal, Greater, Less};

    // A throwaway key, not the release key; these bytes were signed with it.
    const TEST_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDJGNzQ0NEI1NDVCRTRCNTkKUldSWlM3NUZ0VVIwTHlwYzdGVnIxMSs3N0w2YUlxTEJxOExIaUQ3c0UzRGQwSkNtQjZFbmZMTUEK";
    const SIGNED: &[u8] = b"reamesh relay update test data";
    const SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVSWlM3NUZ0VVIwTHdTcy9uL1lyc2pSaC9RK2o2OFhsWC9WR2dLWVhkRityZ2ZLanBwbFRuSklNYnl6RG5GaElOV2JqTmljWlRFUEtVK0R2TFFFeFc3dlAwWk5jL0kvTGd3PQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkzODc0MDc0CWZpbGU6dGVzdGRhdGEKbjdWdDdFR1NBbUxLTUErc2I3dUJaTFFKckhVTDMzaDdvMkFxWEcrNmdFS1pTZm1HejdobHByU3JLcW13SjZycW0wVi9sM2VWQmNJVWU0MmJaM09aQXc9PQo=";

    #[test]
    fn versions_order_like_the_app() {
        assert_eq!(compare_versions("1.2.0", "1.2.0"), Equal);
        assert_eq!(compare_versions("1.10.0", "1.9.9"), Greater);
        assert_eq!(compare_versions("v1.2.0", "1.2.1"), Less);
        assert_eq!(compare_versions("1.2.0-beta.1", "1.2.0"), Less);
        assert_eq!(compare_versions("1.2.0-beta.2", "1.2.0-beta.10"), Less);
        assert_eq!(compare_versions("1.2.0-beta.1", "1.1.9"), Greater);
        assert_eq!(compare_versions("1.2.0-beta", "1.2.0-beta.1"), Less);
    }

    #[test]
    fn the_release_key_is_the_apps_key() {
        let conf = include_str!("../../app/src-tauri/tauri.conf.json");
        assert!(conf.contains(RELEASE_PUBKEY));
        assert!(
            minisign_verify::PublicKey::decode(&{
                use base64::Engine as _;
                String::from_utf8(
                    base64::engine::general_purpose::STANDARD
                        .decode(RELEASE_PUBKEY)
                        .unwrap(),
                )
                .unwrap()
            })
            .is_ok()
        );
    }

    #[test]
    fn signatures_are_checked() {
        verify(TEST_PUBKEY, SIGNED, SIGNATURE).unwrap();
        assert!(verify(TEST_PUBKEY, b"something else", SIGNATURE).is_err());
        assert!(verify(RELEASE_PUBKEY, SIGNED, SIGNATURE).is_err());
        assert!(verify(TEST_PUBKEY, SIGNED, "not base64!").is_err());
    }

    fn manifest(version: &str) -> Manifest {
        serde_json::from_str(&format!(
            r#"{{"version":"{version}","proto":7,"platforms":{{}},"relay":{{"linux-x86_64":{{"url":"https://x/r","signature":"s"}}}}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn only_a_newer_good_build_for_this_machine_is_picked() {
        let m = manifest("1.3.0");
        assert!(pick(&m, "linux-x86_64", "1.2.0", None).is_some());
        assert!(pick(&m, "linux-x86_64", "1.3.0", None).is_none());
        assert!(pick(&m, "linux-x86_64", "1.4.0", None).is_none());
        assert!(pick(&m, "linux-aarch64", "1.2.0", None).is_none());
        assert!(pick(&m, "linux-x86_64", "1.2.0", Some("1.3.0")).is_none());
        // A manifest from before relays were released has no builds at all.
        let old: Manifest = serde_json::from_str(r#"{"version":"9.0.0","platforms":{}}"#).unwrap();
        assert!(pick(&old, "linux-x86_64", "1.2.0", None).is_none());
    }

    #[test]
    fn a_bad_signature_never_gets_staged() {
        let dir = tempfile::tempdir().unwrap();
        assert!(stage(dir.path(), TEST_PUBKEY, "9.0.0", b"tampered", SIGNATURE).is_err());
        assert_eq!(staged_version(dir.path()), None);
    }

    /// Puts files where `stage` would, without running anything.
    fn fake_stage(data: &Path, version: &str, binary: &[u8], sig: &str) {
        let dir = dir_of(data);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(BINARY_FILE), binary).unwrap();
        fs::write(dir.join(SIGNATURE_FILE), sig).unwrap();
        fs::write(dir.join(VERSION_FILE), version).unwrap();
    }

    #[test]
    fn a_copy_that_never_got_ready_is_dropped_and_not_taken_again() {
        let dir = tempfile::tempdir().unwrap();
        fake_stage(dir.path(), "99.0.0", SIGNED, SIGNATURE);
        assert_eq!(pending_version(dir.path()).as_deref(), Some("99.0.0"));
        // First start: go ahead, and remember it is being tried.
        assert!(staged_to_start(dir.path(), TEST_PUBKEY).is_some());
        // It never confirmed, so the next start falls back.
        assert!(staged_to_start(dir.path(), TEST_PUBKEY).is_none());
        assert_eq!(staged_version(dir.path()), None);
        assert_eq!(
            read_trimmed(&dir_of(dir.path()).join(BAD_FILE)).as_deref(),
            Some("99.0.0")
        );
    }

    #[test]
    fn a_copy_that_started_stays_and_a_tampered_or_old_one_goes() {
        let dir = tempfile::tempdir().unwrap();
        fake_stage(dir.path(), "99.0.0", SIGNED, SIGNATURE);
        assert!(staged_to_start(dir.path(), TEST_PUBKEY).is_some());
        confirm_started(dir.path());
        assert!(staged_to_start(dir.path(), TEST_PUBKEY).is_some());

        // Changed on disk after the download.
        fs::write(dir_of(dir.path()).join(BINARY_FILE), b"tampered").unwrap();
        fs::remove_file(dir_of(dir.path()).join(TRYING_FILE)).unwrap();
        assert!(staged_to_start(dir.path(), TEST_PUBKEY).is_none());
        assert_eq!(staged_version(dir.path()), None);

        // Older than what runs: not started, and cleaned up.
        fake_stage(dir.path(), "0.0.1", SIGNED, SIGNATURE);
        assert!(staged_to_start(dir.path(), TEST_PUBKEY).is_none());
        assert_eq!(staged_version(dir.path()), None);

        // The same version as what runs stays: it may be the running copy.
        fake_stage(dir.path(), VERSION, SIGNED, SIGNATURE);
        assert!(staged_to_start(dir.path(), TEST_PUBKEY).is_none());
        assert_eq!(staged_version(dir.path()).as_deref(), Some(VERSION));
    }
}
