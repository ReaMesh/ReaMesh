//! The Windows pieces Setup needs: folders, shortcuts, running processes,
//! the WebView2 check and plain message boxes.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::core::{Interface, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, PROPERTYKEY};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemAlloc, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{IShellLinkW, SHChangeNotify, ShellLink, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MESSAGEBOX_RESULT, MESSAGEBOX_STYLE};

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn local_appdata() -> Option<PathBuf> {
    env_dir("LOCALAPPDATA")
}

pub fn appdata() -> Option<PathBuf> {
    env_dir("APPDATA")
}

/// The current user's Start menu programs folder.
pub fn start_menu() -> Option<PathBuf> {
    appdata().map(|a| a.join(r"Microsoft\Windows\Start Menu\Programs"))
}

/// The desktop, through the shell's own record of where it is (it may be
/// redirected, e.g. into OneDrive).
pub fn desktop() -> Option<PathBuf> {
    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders")
        .ok()?;
    let raw: String = key.get_value("Desktop").ok()?;
    let expanded = expand_env(&raw);
    Some(PathBuf::from(expanded))
}

fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) => {
                let name = &after[..j];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Same file, ignoring case and `\\?\` prefixes.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let s = p.to_string_lossy().replace('/', "\\");
        s.strip_prefix(r"\\?\").unwrap_or(&s).to_lowercase()
    };
    norm(a) == norm(b)
}

// ---------- processes ----------

/// Volume serial + file index: one file however it is spelled (8.3 names,
/// junctions, or a redirected per-app view of AppData).
fn file_id(p: &Path) -> Option<(u32, u32, u32)> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000; // lets a folder be opened
    let f = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(7)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(p)
        .ok()?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(windows::Win32::Foundation::HANDLE(f.as_raw_handle()), &mut info).ok()?;
    }
    Some((info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
}

/// Every running process started from `exe`, including one whose file an
/// earlier run had to park beside it (`.<name>.rm-old-N`).
pub fn processes_of(exe: &Path) -> Vec<u32> {
    let mut out = Vec::new();
    let Some(name) = exe.file_name().map(|n| n.to_string_lossy().to_lowercase()) else {
        return out;
    };
    let parked = format!(".{name}.rm-old-");
    let dir_id = exe.parent().and_then(file_id);
    let matches = |path: &Path| {
        let file = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if file != name && !file.starts_with(&parked) {
            return false;
        }
        if same_path(path, exe) {
            return true;
        }
        dir_id.is_some() && path.parent().and_then(file_id) == dir_id
    };
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut entry).is_ok();
        while ok {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            let image = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
            // The snapshot keeps the name the process started under.
            if image == name {
                if let Some(path) = image_path(entry.th32ProcessID) {
                    if matches(&path) {
                        out.push(entry.th32ProcessID);
                    }
                }
            }
            ok = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

fn image_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(PathBuf::from(OsString::from_wide(&buf[..len as usize])))
    }
}

/// Waits up to `grace` for the processes to end on their own (an updating app
/// is on its way out already), then ends what is left.
pub fn end_processes(exe: &Path, grace: Duration) {
    let until = Instant::now() + grace;
    while !processes_of(exe).is_empty() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    for pid in processes_of(exe) {
        unsafe {
            if let Ok(h) = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) {
                let _ = TerminateProcess(h, 1);
                let _ = WaitForSingleObject(h, 5000);
                let _ = CloseHandle(h);
            }
        }
    }
}

// ---------- shortcuts ----------

/// The property the taskbar groups windows by. The shortcut carries the app's
/// identifier, the same one earlier installers gave it, so pinned shortcuts
/// keep working across the switch.
const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: windows::core::GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3),
    pid: 5,
};

fn com_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

pub fn create_shortcut(lnk: &Path, target: &Path, app_id: &str) -> windows::core::Result<()> {
    com_init();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(target.as_os_str()))?;
        if let Some(dir) = target.parent() {
            link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        }
        link.SetIconLocation(&HSTRING::from(target.as_os_str()), 0)?;
        let store: IPropertyStore = link.cast()?;
        // A VT_LPWSTR PROPVARIANT owns its string as COM memory: the shell
        // frees what it is handed with CoTaskMemFree, so the string can't be
        // a Rust allocation.
        let id: Vec<u16> = app_id.encode_utf16().chain(Some(0)).collect();
        let buf = CoTaskMemAlloc(id.len() * 2) as *mut u16;
        if buf.is_null() {
            return Err(windows::core::Error::from(windows::Win32::Foundation::E_OUTOFMEMORY));
        }
        std::ptr::copy_nonoverlapping(id.as_ptr(), buf, id.len());
        let mut value = PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_LPWSTR,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 { pwszVal: PWSTR(buf) },
                }),
            },
        };
        let set = store.SetValue(&PKEY_APP_USER_MODEL_ID, &value);
        let _ = PropVariantClear(&mut value);
        set?;
        store.Commit()?;
        if let Some(dir) = lnk.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file: IPersistFile = link.cast()?;
        file.Save(&HSTRING::from(lnk.as_os_str()), true)?;
    }
    Ok(())
}

/// Where a shortcut points, if it is one.
pub fn shortcut_target(lnk: &Path) -> Option<PathBuf> {
    if !lnk.is_file() {
        return None;
    }
    com_init();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        file.Load(&HSTRING::from(lnk.as_os_str()), STGM_READ).ok()?;
        let mut buf = vec![0u16; 1024];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(PathBuf::from(OsString::from_wide(&buf[..len])))
    }
}

/// Tells Explorer that URL handlers changed, so a fresh `reamesh://` handler
/// works without signing out.
pub fn notify_associations() {
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

// ---------- WebView2 ----------

const WEBVIEW2_GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

/// The installed WebView2 Runtime's version, where the Edge updater records
/// it (machine-wide or per user).
pub fn webview2_version() -> Option<String> {
    use winreg::enums::*;
    use winreg::RegKey;
    let paths = [
        (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_GUID}")),
        (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_GUID}")),
        (HKEY_CURRENT_USER, format!(r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_GUID}")),
    ];
    for (root, path) in paths {
        if let Ok(key) = RegKey::predef(root).open_subkey(&path) {
            if let Ok(v) = key.get_value::<String, _>("pv") {
                if !v.is_empty() && v != "0.0.0.0" {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// Downloads Microsoft's WebView2 bootstrapper with the system's curl and runs
/// it. Only reached on a Windows 10 that never received the runtime.
pub fn install_webview2() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let dir = std::env::temp_dir().join(format!("reamesh-webview2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let exe = dir.join("MicrosoftEdgeWebview2Setup.exe");
    let system = env_dir("SystemRoot").unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let ok = std::process::Command::new(system.join(r"System32\curl.exe"))
        .args(["-fsSL", "--retry", "2", "-o"])
        .arg(&exe)
        .arg("https://go.microsoft.com/fwlink/p/?LinkId=2124703")
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        let _ = std::fs::remove_dir_all(&dir);
        return Err("download failed".into());
    }
    let status = std::process::Command::new(&exe).args(["/silent", "/install"]).status();
    let _ = std::fs::remove_dir_all(&dir);
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("the WebView2 installer stopped with {s}")),
        Err(e) => Err(e.to_string()),
    }
}

// ---------- misc ----------

pub fn message_box(text: &str, flags: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from("ReaMesh Setup"), flags) }
}

/// The process's command line exactly as it was passed, for handing the
/// updater's `/ARGS` tail on to the app untouched.
pub fn raw_command_line() -> String {
    unsafe {
        let p = windows::Win32::System::Environment::GetCommandLineW();
        if p.is_null() {
            return String::new();
        }
        p.to_string().unwrap_or_default()
    }
}

/// Free bytes on the disk holding `dir` (its nearest existing ancestor).
pub fn free_space(dir: &Path) -> Option<u64> {
    let mut d = dir;
    while !d.exists() {
        d = d.parent()?;
    }
    let mut free = 0u64;
    unsafe {
        windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            PCWSTR(wide(d).as_ptr()),
            Some(&mut free),
            None,
            None,
        )
        .ok()?;
    }
    Some(free)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands() {
        std::env::set_var("RMSETUP_X", "C:\\Users\\a");
        assert_eq!(expand_env("%RMSETUP_X%\\Desktop"), "C:\\Users\\a\\Desktop");
        assert_eq!(expand_env("100%"), "100%");
        assert_eq!(expand_env("%NOPE_RMSETUP%\\x"), "%NOPE_RMSETUP%\\x");
    }

    #[test]
    fn paths() {
        assert!(same_path(Path::new(r"C:\A\app.exe"), Path::new(r"\\?\c:\a\APP.exe")));
        assert!(!same_path(Path::new(r"C:\A\app.exe"), Path::new(r"C:\B\app.exe")));
    }
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    #[test]
    fn shortcut_round_trip() {
        let dir = std::env::temp_dir().join(format!("rmsetup-lnk-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let lnk = dir.join("x.lnk");
        let target = std::env::current_exe().unwrap();
        create_shortcut(&lnk, &target, "com.reamesh.Test").unwrap();
        let got = shortcut_target(&lnk).unwrap();
        assert!(same_path(&got, &target), "{got:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}


#[cfg(test)]
mod process_tests {
    use super::*;

    /// A copy of ping.exe stands in for the app: found by its path, still
    /// found once parked under another name, then ended.
    #[test]
    fn finds_and_ends() {
        let dir = std::env::temp_dir().join(format!("rmsetup-proc-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let exe = dir.join("app.exe");
        let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap();
        std::fs::copy(system.join(r"System32\PING.EXE"), &exe).unwrap();
        let mut child = std::process::Command::new(&exe)
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(processes_of(&exe), vec![child.id()]);
        std::fs::rename(&exe, dir.join(".app.exe.rm-old-0")).unwrap();
        assert_eq!(processes_of(&exe), vec![child.id()]);
        end_processes(&exe, Duration::from_millis(0));
        assert!(child.wait().is_ok());
        assert!(processes_of(&exe).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
