//! What Setup does to the machine: install, update, uninstall.
//!
//! The layout is the one the earlier NSIS installer used, so Setup updates
//! those installs in place and the app's own updater keeps working across the
//! switch: a per-user install in `%LOCALAPPDATA%\ReaMesh` (or wherever the
//! previous install went), `app.exe` + `reaper_reamesh.dll` + `uninstall.exe`,
//! the uninstall entry under `HKCU\…\Uninstall\ReaMesh`, the `reamesh://`
//! handler and a Start menu shortcut. Nothing needs administrator rights.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
use winreg::RegKey;

use crate::payload::{self, Header, Located};
use crate::win;

pub const PRODUCT: &str = "ReaMesh";
pub const MAIN_BINARY: &str = "app.exe";
pub const UNINSTALLER: &str = "uninstall.exe";
/// The app's identifier: its settings folders and the taskbar id.
pub const APP_ID: &str = "com.reamesh.ReaMesh";
const PLUGIN: &str = "reaper_reamesh.dll";
const UNINST_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\ReaMesh";
/// Where the NSIS installer kept the install folder; still written so either
/// installer finds the other's install.
const MANU_KEY: &str = r"Software\reamesh";
const MANU_PRODUCT_KEY: &str = r"Software\reamesh\ReaMesh";
const URL_KEY: &str = r"Software\Classes\reamesh";

fn hkcu() -> RegKey {
    RegKey::predef(HKEY_CURRENT_USER)
}

/// An existing install, from its uninstall entry.
#[derive(Clone, Debug)]
pub struct Installed {
    pub version: Option<String>,
    pub dir: PathBuf,
}

pub fn installed() -> Option<Installed> {
    let unquote = |s: String| s.trim().trim_matches('"').to_string();
    let entry = hkcu().open_subkey_with_flags(UNINST_KEY, KEY_READ).ok();
    let dir = entry
        .as_ref()
        .and_then(|k| k.get_value::<String, _>("InstallLocation").ok())
        .map(unquote)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            hkcu()
                .open_subkey_with_flags(MANU_PRODUCT_KEY, KEY_READ)
                .ok()?
                .get_value::<String, _>("")
                .ok()
                .map(unquote)
                .filter(|s| !s.is_empty())
        })?;
    let version = entry
        .as_ref()
        .and_then(|k| k.get_value::<String, _>("DisplayVersion").ok())
        .filter(|s| !s.is_empty());
    Some(Installed { version, dir: PathBuf::from(dir) })
}

pub fn default_dir() -> PathBuf {
    win::local_appdata()
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"))
        .join(PRODUCT)
}

/// A folder the user picked: used as is when it is empty or already holds
/// ReaMesh, otherwise ReaMesh gets its own folder inside it (an uninstall
/// never deletes anything it didn't put there, but a folder of its own keeps
/// the two apart).
pub fn normalize_dir(picked: &Path) -> PathBuf {
    let holds_app = picked.join(MAIN_BINARY).is_file();
    let empty = fs::read_dir(picked).map(|mut d| d.next().is_none()).unwrap_or(true);
    let named = picked.file_name().is_some_and(|n| n.eq_ignore_ascii_case(PRODUCT));
    if holds_app || empty || named {
        picked.to_path_buf()
    } else {
        picked.join(PRODUCT)
    }
}

pub fn app_running(dir: &Path) -> bool {
    !win::processes_of(&dir.join(MAIN_BINARY)).is_empty()
}

/// -1 / 0 / 1 for a < b / a == b / a > b; a pre-release sorts before its
/// release.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    fn split(v: &str) -> (Vec<u64>, Option<&str>) {
        let v = v.trim().trim_start_matches('v');
        let (core, pre) = match v.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (v, None),
        };
        (core.split('.').map(|p| p.parse().unwrap_or(0)).collect(), pre)
    }
    let (ca, pa) = split(a);
    let (cb, pb) = split(b);
    let n = ca.len().max(cb.len());
    for i in 0..n {
        let x = ca.get(i).copied().unwrap_or(0);
        let y = cb.get(i).copied().unwrap_or(0);
        if x != y {
            return x.cmp(&y);
        }
    }
    match (pa, pb) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(x), Some(y)) => x.cmp(y),
    }
}

// ---------- install ----------

pub struct InstallOpts {
    pub dir: PathBuf,
    /// Create a Start menu shortcut (when missing).
    pub start_menu: bool,
    pub desktop: bool,
    /// Run by the app's updater: the app is quitting on its own.
    pub update: bool,
}

/// One step of the work, for the progress line.
#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Verify,
    Close,
    Copy,
    Register,
    Remove,
    Done,
}

pub type Progress<'a> = &'a mut dyn FnMut(Phase, f64);

pub fn install(exe: &Path, at: &Located, opts: &InstallOpts, progress: Progress) -> Result<Header, String> {
    progress(Phase::Verify, 0.0);
    let blob = payload::read_blob(exe, at).map_err(|e| e.to_string())?;
    let header = payload::read_header(&blob).map_err(|e| e.to_string())?;
    let total = header.total().max(1);
    if let Some(free) = win::free_space(&opts.dir) {
        if free < total + (64 << 20) {
            return Err(format!(
                "not enough disk space: {} MB free, {} MB needed",
                free >> 20,
                (total + (64 << 20)) >> 20
            ));
        }
    }
    progress(Phase::Verify, 1.0);

    progress(Phase::Close, 0.0);
    let main = opts.dir.join(MAIN_BINARY);
    win::end_processes(&main, if opts.update { Duration::from_secs(10) } else { Duration::from_millis(300) });
    progress(Phase::Close, 1.0);

    fs::create_dir_all(&opts.dir).map_err(|e| format!("{}: {e}", opts.dir.display()))?;
    sweep_parked(&opts.dir);

    // Every file lands under a temporary name first and replaces the old one
    // only once it is complete.
    let mut staged: Vec<(PathBuf, PathBuf)> = Vec::new();
    let result = (|| -> io::Result<()> {
        payload::extract(
            &blob,
            |entry| {
                let dest = opts.dir.join(&entry.path);
                let tmp = opts.dir.join(format!(".{}.rm-new", entry.path));
                staged.push((tmp.clone(), dest));
                Ok(Box::new(io::BufWriter::new(fs::File::create(&tmp)?)) as Box<dyn Write>)
            },
            |done| progress(Phase::Copy, done as f64 / total as f64),
        )?;
        // The uninstaller is this very program without its payload.
        let tmp = opts.dir.join(format!(".{UNINSTALLER}.rm-new"));
        {
            let mut src = fs::File::open(exe)?;
            let mut out = io::BufWriter::new(fs::File::create(&tmp)?);
            io::copy(&mut io::Read::take(&mut src, at.stub_len), &mut out)?;
            out.flush()?;
        }
        staged.push((tmp, opts.dir.join(UNINSTALLER)));
        Ok(())
    })();
    if let Err(e) = result {
        for (tmp, _) in &staged {
            let _ = fs::remove_file(tmp);
        }
        return Err(e.to_string());
    }
    for (tmp, dest) in &staged {
        if let Err(e) = replace(tmp, dest) {
            for (t, _) in &staged {
                let _ = fs::remove_file(t);
            }
            return Err(format!("{}: {e}", dest.display()));
        }
    }

    progress(Phase::Register, 0.0);
    register(opts, &header).map_err(|e| format!("registry: {e}"))?;
    shortcuts(opts);
    win::notify_associations();
    progress(Phase::Register, 1.0);
    progress(Phase::Done, 1.0);
    Ok(header)
}

/// Moves `tmp` over `dest`. A `dest` Windows won't let go of (still mapped
/// by a process that didn't end) is renamed out of the way first; running
/// files can be renamed, just not overwritten.
fn replace(tmp: &Path, dest: &Path) -> io::Result<()> {
    if fs::rename(tmp, dest).is_ok() {
        return Ok(());
    }
    if dest.exists() {
        let dir = dest.parent().unwrap_or(Path::new("."));
        let name = dest.file_name().unwrap_or_default().to_string_lossy();
        let mut n = 0;
        let parked = loop {
            let p = dir.join(format!(".{name}.rm-old-{n}"));
            if !p.exists() {
                break p;
            }
            n += 1;
        };
        fs::rename(dest, &parked)?;
    }
    fs::rename(tmp, dest)
}

/// Removes files earlier runs had to park, and their half-written temps.
fn sweep_parked(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && (name.contains(".rm-old-") || name.ends_with(".rm-new")) {
            let _ = fs::remove_file(e.path());
        }
    }
}

fn register(opts: &InstallOpts, header: &Header) -> io::Result<()> {
    let dir = &opts.dir;
    let main = dir.join(MAIN_BINARY);
    let main_s = main.to_string_lossy();

    let (url, _) = hkcu().create_subkey(URL_KEY)?;
    url.set_value("", &format!("URL:{APP_ID} protocol"))?;
    url.set_value("URL Protocol", &"")?;
    let (icon, _) = url.create_subkey("DefaultIcon")?;
    icon.set_value("", &format!("\"{main_s}\",0"))?;
    let (cmd, _) = url.create_subkey(r"shell\open\command")?;
    cmd.set_value("", &format!("\"{main_s}\" \"%1\""))?;

    let (manu, _) = hkcu().create_subkey(MANU_PRODUCT_KEY)?;
    manu.set_value("", &dir.to_string_lossy().as_ref())?;

    let (key, _) = hkcu().create_subkey(UNINST_KEY)?;
    let uninst = dir.join(UNINSTALLER);
    key.set_value("DisplayName", &PRODUCT)?;
    key.set_value("DisplayIcon", &format!("\"{main_s}\""))?;
    key.set_value("DisplayVersion", &header.version)?;
    key.set_value("Publisher", &PRODUCT)?;
    key.set_value("URLInfoAbout", &"https://github.com/ReaMesh/ReaMesh")?;
    key.set_value("InstallLocation", &format!("\"{}\"", dir.display()))?;
    key.set_value("UninstallString", &format!("\"{}\"", uninst.display()))?;
    key.set_value("QuietUninstallString", &format!("\"{}\" /S", uninst.display()))?;
    key.set_value("MainBinaryName", &MAIN_BINARY)?;
    key.set_value("NoModify", &1u32)?;
    key.set_value("NoRepair", &1u32)?;
    let mut files: Vec<String> = header.files.iter().map(|f| f.path.clone()).collect();
    files.push(UNINSTALLER.into());
    key.set_value("InstalledFiles", &files.join("|"))?;
    let kb: u64 = files
        .iter()
        .filter_map(|f| fs::metadata(dir.join(f)).ok())
        .map(|m| m.len())
        .sum::<u64>()
        / 1024;
    key.set_value("EstimatedSize", &(kb.min(u32::MAX as u64) as u32))?;
    Ok(())
}

fn shortcut_paths() -> (Option<PathBuf>, Option<PathBuf>) {
    let lnk = format!("{PRODUCT}.lnk");
    (win::start_menu().map(|d| d.join(&lnk)), win::desktop().map(|d| d.join(&lnk)))
}

fn shortcuts(opts: &InstallOpts) {
    let main = opts.dir.join(MAIN_BINARY);
    let (start, desktop) = shortcut_paths();
    for (path, wanted) in [(start, opts.start_menu), (desktop, opts.desktop)] {
        let Some(path) = path else { continue };
        let existing = win::shortcut_target(&path);
        // Ours already (an update): refreshed so it carries the current icon.
        // Missing: created when asked for. Someone else's: left alone.
        let ours = existing.as_deref().is_some_and(|t| win::same_path(t, &main));
        if ours || (existing.is_none() && wanted) {
            let _ = win::create_shortcut(&path, &main, APP_ID);
        }
    }
}

// ---------- uninstall ----------

pub struct UninstallOpts {
    pub dir: PathBuf,
    pub remove_extension: bool,
    pub remove_data: bool,
}

#[derive(Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallResult {
    /// REAPER still has the extension loaded until it restarts.
    pub restart_reaper: bool,
}

fn app_config_dir() -> Option<PathBuf> {
    win::appdata().map(|a| a.join(APP_ID))
}

/// REAPER's resource folder as the app knows it: the folder chosen in its
/// settings, else REAPER's default.
pub fn reaper_dir() -> Option<PathBuf> {
    let custom = app_config_dir()
        .and_then(|d| fs::read_to_string(d.join("reaper_path")).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    custom.or_else(|| win::appdata().map(|a| a.join("REAPER")))
}

pub fn extension_installed() -> bool {
    reaper_dir().is_some_and(|d| d.join("UserPlugins").join(PLUGIN).is_file())
}

pub fn data_exists() -> bool {
    app_config_dir().is_some_and(|d| d.exists())
        || win::local_appdata().is_some_and(|d| d.join(APP_ID).exists())
}

pub fn uninstall(opts: &UninstallOpts, progress: Progress) -> Result<UninstallResult, String> {
    let mut result = UninstallResult::default();
    let dir = &opts.dir;
    let main = dir.join(MAIN_BINARY);

    progress(Phase::Close, 0.0);
    win::end_processes(&main, Duration::from_millis(300));
    progress(Phase::Close, 1.0);

    // Read before the settings folder may go: it names the REAPER folder.
    let reaper = reaper_dir();

    progress(Phase::Remove, 0.0);
    let listed = hkcu()
        .open_subkey_with_flags(UNINST_KEY, KEY_READ)
        .ok()
        .and_then(|k| k.get_value::<String, _>("InstalledFiles").ok());
    let mut files: Vec<String> = listed
        .map(|s| s.split('|').filter(|f| payload::safe_name(f)).map(String::from).collect())
        .unwrap_or_default();
    for f in [MAIN_BINARY, PLUGIN] {
        if !files.iter().any(|x| x.eq_ignore_ascii_case(f)) {
            files.push(f.into());
        }
    }
    let me = std::env::current_exe().ok();
    for f in &files {
        let p = dir.join(f);
        // The uninstaller runs from here; it goes once this process is done.
        if me.as_deref().is_some_and(|m| win::same_path(m, &p)) {
            continue;
        }
        let _ = fs::remove_file(&p);
    }
    sweep_parked(dir);
    progress(Phase::Remove, 0.4);

    let (start, desktop) = shortcut_paths();
    for lnk in [start, desktop].into_iter().flatten() {
        if win::shortcut_target(&lnk).is_some_and(|t| win::same_path(&t, &main)) {
            let _ = fs::remove_file(&lnk);
        }
    }
    let cmd: Option<String> = hkcu()
        .open_subkey_with_flags(format!(r"{URL_KEY}\shell\open\command"), KEY_READ)
        .ok()
        .and_then(|k| k.get_value("").ok());
    if cmd.is_some_and(|c| c.to_lowercase().contains(&main.to_string_lossy().to_lowercase())) {
        let _ = hkcu().delete_subkey_all(URL_KEY);
    }
    let _ = hkcu().delete_subkey_all(UNINST_KEY);
    let _ = hkcu().delete_subkey_all(MANU_PRODUCT_KEY);
    let _ = hkcu().delete_subkey(MANU_KEY); // only when empty
    let _ = hkcu().delete_subkey_all(format!(r"Software\Classes\AppUserModelId\{APP_ID}"));
    if let Ok(run) = hkcu().open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        winreg::enums::KEY_SET_VALUE,
    ) {
        let _ = run.delete_value(PRODUCT);
    }
    win::notify_associations();
    progress(Phase::Remove, 0.6);

    if opts.remove_extension {
        if let Some(reaper) = reaper {
            result.restart_reaper = remove_extension(&reaper.join("UserPlugins"));
        }
    }
    progress(Phase::Remove, 0.8);

    if opts.remove_data {
        if let Some(d) = app_config_dir() {
            let _ = fs::remove_dir_all(d);
        }
        if let Some(d) = win::local_appdata() {
            let _ = fs::remove_dir_all(d.join(APP_ID));
        }
    }
    progress(Phase::Remove, 1.0);
    progress(Phase::Done, 1.0);
    Ok(result)
}

/// Deletes the extension from REAPER's UserPlugins. One REAPER still has
/// loaded can't be deleted, only renamed: it is parked under a name REAPER
/// never loads (`reaper_*.dll` only) and stays loaded until REAPER quits.
/// Returns whether that happened.
fn remove_extension(user_plugins: &Path) -> bool {
    let mut loaded = false;
    let dll = user_plugins.join(PLUGIN);
    if dll.exists() && fs::remove_file(&dll).is_err() {
        let mut n = 0;
        let parked = loop {
            let p = user_plugins.join(format!(".{PLUGIN}.old-{n}"));
            if !p.exists() {
                break p;
            }
            n += 1;
        };
        loaded = fs::rename(&dll, parked).is_ok();
    }
    // Copies an earlier app install parked the same way.
    if let Ok(entries) = fs::read_dir(user_plugins) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with(&format!(".{PLUGIN}.old-")) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    loaded
}

/// Removes what can't go while this process runs a moment after it has
/// exited: `files` (the uninstaller itself), `if_empty` (the install folder:
/// never anything the user put there) and `trees` (the webview's scratch
/// folder, Setup's own).
pub fn clean_up_after_exit(files: &[PathBuf], if_empty: &[PathBuf], trees: &[PathBuf]) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    if files.is_empty() && if_empty.is_empty() && trees.is_empty() {
        return;
    }
    let mut script = String::from("ping -n 3 127.0.0.1 >nul");
    for f in files {
        script.push_str(&format!(" & del /f /q \"{}\"", f.display()));
    }
    for d in if_empty {
        script.push_str(&format!(" & rmdir /q \"{}\"", d.display()));
    }
    for d in trees {
        script.push_str(&format!(" & rmdir /s /q \"{}\"", d.display()));
    }
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let _ = std::process::Command::new(system.join(r"System32\cmd.exe"))
        .raw_arg(format!("/d /s /c \"{script} >nul 2>&1\""))
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .current_dir(std::env::temp_dir())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::*;

    #[test]
    fn versions() {
        assert_eq!(compare_versions("1.3.0", "1.2.9"), Greater);
        assert_eq!(compare_versions("1.3.0", "1.3.0"), Equal);
        assert_eq!(compare_versions("1.3", "1.3.0"), Equal);
        assert_eq!(compare_versions("1.4.0-beta.1", "1.4.0"), Less);
        assert_eq!(compare_versions("1.10.0", "1.9.0"), Greater);
        assert_eq!(compare_versions("v2.0.0", "1.99.0"), Greater);
    }

    #[test]
    fn replaces_and_parks() {
        let dir = std::env::temp_dir().join(format!("rmsetup-ops-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("a.txt");
        fs::write(&dest, "old").unwrap();
        let tmp = dir.join(".a.txt.rm-new");
        fs::write(&tmp, "new").unwrap();
        replace(&tmp, &dest).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "new");
        assert!(!tmp.exists());
        fs::write(dir.join(".a.txt.rm-old-0"), "x").unwrap();
        sweep_parked(&dir);
        assert!(!dir.join(".a.txt.rm-old-0").exists());
        assert!(dest.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn picked_folders() {
        let dir = std::env::temp_dir().join(format!("rmsetup-pick-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(normalize_dir(&dir), dir);
        fs::write(dir.join("other.txt"), "").unwrap();
        assert_eq!(normalize_dir(&dir), dir.join(PRODUCT));
        fs::write(dir.join(MAIN_BINARY), "").unwrap();
        assert_eq!(normalize_dir(&dir), dir);
        let _ = fs::remove_dir_all(&dir);
    }
}
