// "Save as" for a downloaded copy: a joined session's folder goes to a folder the
// user picks, on a background thread, with the progress as events. The copy is
// built under an incomplete name and only gets its real name once every byte is
// on the disk, so a folder with the real name is always a whole copy. It never
// takes over an existing folder: a taken name becomes "Name 2".
//
// A copy that fails goes to the Trash. Where that isn't possible it stays where
// it is under its incomplete name, and the error says so. So does a whole copy
// that can't get its name. Nothing is deleted. A finished copy brings the folder's
// `.reamesh-link.json` along, which is what links it (see links.rs).

use std::fs::{self, File};
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};

/// Free space a copy leaves on the target disk.
const COPY_RESERVE: u64 = 16 << 20;
const COPY_BUFFER: usize = 1 << 20;
const PROGRESS_EVERY: Duration = Duration::from_millis(100);
/// What a copy is called until it is whole.
const INCOMPLETE_SUFFIX: &str = ".reamesh-incomplete";
/// How many names ("Name", "Name 2", …) a copy tries.
const NAME_ATTEMPTS: u32 = 1000;
/// How long a finished copy keeps trying for its name, about ten seconds.
const PUBLISH_ATTEMPTS: u32 = 40;
const PUBLISH_PAUSE: Duration = Duration::from_millis(250);

/// Not worth keeping in a copy: transfer temp files, the stripped sibling a join
/// without effects opens, and the mark that keeps a folder in the staging root,
/// which means nothing anywhere else. Other dotfiles travel along.
fn skip_in_copy(name: &str) -> bool {
    name.starts_with(".reamesh-part-") || name == ".reamesh-nofx.rpp" || name == ".reamesh-keep"
}

#[derive(Debug, PartialEq)]
pub(crate) enum Partial {
    /// Nothing was written.
    None,
    Trashed,
    /// Couldn't go to the Trash: it stays under this incomplete name.
    Kept(PathBuf),
}

#[derive(Debug, PartialEq)]
pub(crate) enum Why {
    /// The target disk has no room for it, checked up front or hit on the way.
    NoSpace,
    /// A folder inside the staging root, which the sweep empties.
    InsideStaging,
    /// As the OS reports it.
    Io(String),
}

#[derive(Debug, PartialEq)]
pub(crate) struct Failed {
    pub why: Why,
    pub partial: Partial,
}

fn classify(e: io::Error) -> Why {
    match e.kind() {
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => Why::NoSpace,
        _ => Why::Io(e.to_string()),
    }
}

/// What the copy needs from the outside, so a test can run it on a disk that
/// fills up without filling one.
pub(crate) struct Hooks<'a> {
    pub free: &'a dyn Fn(&Path) -> Option<u64>,
    pub write: &'a mut dyn FnMut(&mut File, &[u8]) -> io::Result<()>,
    /// Move a failed copy to the Trash; false when that isn't possible.
    pub discard: &'a mut dyn FnMut(&Path) -> bool,
    /// (bytes done, bytes in total)
    pub progress: &'a mut dyn FnMut(u64, u64),
}

/// Files and folders to copy, as paths relative to `source`. An unreadable folder
/// is an error rather than a gap in the copy. Links are left out: a link out of
/// the folder isn't part of it.
fn plan(source: &Path) -> io::Result<(Vec<PathBuf>, Vec<PathBuf>, u64)> {
    let (mut dirs, mut files, mut total) = (Vec::new(), Vec::new(), 0u64);
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for entry in fs::read_dir(source.join(&rel))? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let child = rel.join(entry.file_name());
            if kind.is_dir() {
                dirs.push(child.clone());
                stack.push(child);
            } else if kind.is_file() && !entry.file_name().to_str().is_some_and(skip_in_copy) {
                total = total.saturating_add(entry.metadata()?.len());
                files.push(child);
            }
        }
    }
    // Parents before their children.
    dirs.sort();
    Ok((dirs, files, total))
}

/// The first free name of "Name", "Name 2", "Name 3", …, as far as anyone can
/// tell before actually taking it. Anything but "not found" counts as taken.
fn free_name(dest: &Path, base: &str, suffix: &str) -> Option<PathBuf> {
    (1..=NAME_ATTEMPTS).find_map(|n| {
        let name = if n == 1 { format!("{base}{suffix}") } else { format!("{base} {n}{suffix}") };
        let path = dest.join(name);
        match path.symlink_metadata() {
            Err(e) if e.kind() == ErrorKind::NotFound => Some(path),
            _ => None,
        }
    })
}

/// Make the incomplete folder. `create_dir` fails on a name someone just took.
fn make_incomplete(dest: &Path, name: &str) -> io::Result<PathBuf> {
    for _ in 0..NAME_ATTEMPTS {
        let Some(path) = free_name(dest, name, INCOMPLETE_SUFFIX) else { break };
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(ErrorKind::AlreadyExists, "no free folder name"))
}

/// Give the finished copy its real name. Taking an existing name is impossible:
/// a rename fails on a folder that has anything in it, and a name that turned up
/// a moment ago just means the next one. On Windows a program with one of the new
/// files open (a virus scanner, usually) keeps the folder from being renamed: that
/// passes, so it waits.
fn publish(incomplete: &Path, dest: &Path, name: &str) -> io::Result<PathBuf> {
    let mut last = None;
    for _ in 0..PUBLISH_ATTEMPTS {
        let Some(target) = free_name(dest, name, "") else { break };
        match fs::rename(incomplete, &target) {
            Ok(()) => return Ok(target),
            Err(e) if e.kind() == ErrorKind::PermissionDenied || target.symlink_metadata().is_ok() => {
                last = Some(e);
            }
            Err(e) => return Err(e),
        }
        std::thread::sleep(PUBLISH_PAUSE);
    }
    Err(last.unwrap_or_else(|| io::Error::new(ErrorKind::AlreadyExists, "no free folder name")))
}

fn copy_file(
    from: &Path,
    to: &Path,
    buf: &mut [u8],
    mut wrote: impl FnMut(usize),
    write: &mut dyn FnMut(&mut File, &[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let mut src = File::open(from)?;
    let meta = src.metadata()?;
    let mut out = File::create_new(to)?;
    loop {
        let n = src.read(buf)?;
        if n == 0 {
            break;
        }
        write(&mut out, &buf[..n])?;
        wrote(n);
    }
    // Peak files are tied to their source's time.
    if let Ok(t) = meta.modified() {
        let _ = out.set_modified(t);
    }
    // A full disk or a pulled drive can show only here.
    out.sync_all()
}

/// Copy `source` to `dest/<name>` (or the next free name), returns where it went.
pub(crate) fn save_copy(
    source: &Path,
    name: &str,
    dest: &Path,
    staging_root: &Path,
    hooks: Hooks,
) -> Result<PathBuf, Failed> {
    let Hooks { free, write, discard, progress } = hooks;
    let fail = |why| Failed { why, partial: Partial::None };
    if !fs::metadata(dest).map_err(|e| fail(classify(e)))?.is_dir() {
        return Err(fail(Why::Io("not a folder".into())));
    }
    // Inside the staging root it would be swept like a downloaded copy.
    if let (Ok(d), Ok(r)) = (dest.canonicalize(), staging_root.canonicalize()) {
        if d.starts_with(r) {
            return Err(fail(Why::InsideStaging));
        }
    }
    let (dirs, files, total) = plan(source).map_err(|e| fail(classify(e)))?;
    if free(dest).is_some_and(|f| total.saturating_add(COPY_RESERVE) > f) {
        return Err(fail(Why::NoSpace));
    }
    let incomplete = make_incomplete(dest, name).map_err(|e| fail(classify(e)))?;

    let copied = (|| -> io::Result<()> {
        for rel in &dirs {
            fs::create_dir(incomplete.join(rel))?;
        }
        let mut buf = vec![0u8; COPY_BUFFER];
        let (mut done, mut last) = (0u64, Instant::now());
        progress(0, total);
        for rel in &files {
            copy_file(
                &source.join(rel),
                &incomplete.join(rel),
                &mut buf,
                |n| {
                    done = done.saturating_add(n as u64);
                    if last.elapsed() >= PROGRESS_EVERY {
                        last = Instant::now();
                        progress(done, total);
                    }
                },
                write,
            )?;
        }
        progress(done, total);
        Ok(())
    })();
    if let Err(e) = copied {
        let partial = if discard(&incomplete) { Partial::Trashed } else { Partial::Kept(incomplete) };
        return Err(Failed { why: classify(e), partial });
    }
    // Whole by now, so whatever goes wrong here it isn't thrown away.
    publish(&incomplete, dest, name).map_err(|e| Failed { why: classify(e), partial: Partial::Kept(incomplete.clone()) })
}

// ---------- the running copy ----------

static COPYING: AtomicBool = AtomicBool::new(false);

/// One copy at a time. Held by the copy's thread, whatever way that ends.
struct CopySlot;

impl CopySlot {
    fn take() -> Option<Self> {
        COPYING.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).ok().map(|_| CopySlot)
    }
}

impl Drop for CopySlot {
    fn drop(&mut self) {
        COPYING.store(false, Ordering::Release);
    }
}

#[derive(Clone, serde::Serialize)]
struct CopyProgress<'a> {
    dir: &'a str,
    done: u64,
    total: u64,
}

/// `error` is "no_space", "inside_staging" or the OS's own words. A failed copy
/// says where its remains are: `trashed`, or `kept` under an incomplete name.
#[derive(Clone, serde::Serialize)]
struct CopyFinished {
    dir: String,
    path: Option<String>,
    error: Option<String>,
    trashed: bool,
    kept: Option<String>,
    /// The new folder is a linked copy of the project from now on.
    linked: bool,
}

fn finished(dir: String, result: Result<PathBuf, Failed>) -> CopyFinished {
    let mut out = CopyFinished { dir, path: None, error: None, trashed: false, kept: None, linked: false };
    match result {
        Ok(path) => out.path = Some(path.to_string_lossy().into_owned()),
        Err(Failed { why, partial }) => {
            out.error = Some(match why {
                Why::NoSpace => "no_space".into(),
                Why::InsideStaging => "inside_staging".into(),
                Why::Io(text) => text,
            });
            match partial {
                Partial::None => {}
                Partial::Trashed => out.trashed = true,
                Partial::Kept(p) => out.kept = Some(p.to_string_lossy().into_owned()),
            }
        }
    }
    out
}

/// Start copying on a thread of its own; the rest arrives as `copy-progress` and
/// `copy-finished`. Errs only when it can't start: another copy is running.
pub(crate) fn start(
    app: AppHandle,
    dir: String,
    name: String,
    source: PathBuf,
    dest: PathBuf,
    staging_root: PathBuf,
) -> Result<(), String> {
    let slot = CopySlot::take().ok_or("another copy is running")?;
    std::thread::Builder::new()
        .name("joined-copy".into())
        .spawn(move || {
            let _slot = slot;
            let result = save_copy(
                &source,
                &name,
                &dest,
                &staging_root,
                Hooks {
                    free: &available_space,
                    write: &mut |f, b| f.write_all(b),
                    discard: &mut |p| crate::move_to_trash(p).is_ok(),
                    progress: &mut |done, total| {
                        let _ = app.emit("copy-progress", CopyProgress { dir: &dir, done, total });
                    },
                },
            );
            // A whole copy that knows what it is a copy of is linked from here on.
            let linked = result.as_ref().is_ok_and(|path| crate::links::remember(&app, path));
            let mut end = finished(dir.clone(), result);
            end.linked = linked;
            let _ = app.emit("copy-finished", end);
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Bytes available to this user on the filesystem holding `dir`, `None` when it
/// can't be told (then nothing is refused for lack of space up front).
#[cfg(unix)]
fn available_space(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: a zeroed statvfs is a valid out-parameter; `c` is NUL-terminated.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}

#[cfg(windows)]
fn available_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut avail = 0u64;
    // SAFETY: `wide` is NUL-terminated; the two null out-parameters are optional.
    let ok = unsafe {
        GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, std::ptr::null_mut(), std::ptr::null_mut())
    };
    (ok != 0).then_some(avail)
}

#[cfg(not(any(unix, windows)))]
fn available_space(_dir: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    /// A scratch tree: `root/staging/Song-0f3a` to copy from, `root/out` to copy to.
    struct Tree {
        root: PathBuf,
        source: PathBuf,
        staging: PathBuf,
        out: PathBuf,
    }

    impl Tree {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let root = std::env::temp_dir().join(format!(
                "reamesh-copies-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let staging = root.join("staging");
            let source = staging.join("Song-0f3a");
            let out = root.join("out");
            fs::create_dir_all(source.join("Audio/Takes")).unwrap();
            fs::create_dir_all(&out).unwrap();
            fs::write(source.join("Song.rpp"), b"<REAPER_PROJECT\n>\n").unwrap();
            fs::write(source.join("Audio/a.wav"), vec![7u8; 300_000]).unwrap();
            fs::write(source.join("Audio/Takes/b.wav"), vec![9u8; 1000]).unwrap();
            fs::write(source.join(".reamesh-part-1234"), b"half").unwrap();
            fs::write(source.join(".reamesh-nofx.rpp"), b"stripped").unwrap();
            fs::write(source.join(".reamesh-link.json"), b"{}").unwrap();
            fs::write(source.join(".reamesh-keep"), b"").unwrap();
            Tree { root, source, staging, out }
        }

        fn run(&self, write: &mut dyn FnMut(&mut File, &[u8]) -> io::Result<()>, discard: &mut dyn FnMut(&Path) -> bool) -> Result<PathBuf, Failed> {
            save_copy(
                &self.source,
                "Song",
                &self.out,
                &self.staging,
                Hooks { free: &|_| None, write, discard, progress: &mut |_, _| {} },
            )
        }

        fn copy(&self) -> Result<PathBuf, Failed> {
            self.run(&mut |f, b| f.write_all(b), &mut |_| panic!("nothing to discard"))
        }

        fn out_names(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.out)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn copies_the_project_and_leaves_out_the_session_temp_files() {
        let t = Tree::new();
        let path = t.copy().unwrap();
        assert_eq!(path, t.out.join("Song"));
        assert_eq!(fs::read(path.join("Song.rpp")).unwrap(), b"<REAPER_PROJECT\n>\n");
        assert_eq!(fs::read(path.join("Audio/a.wav")).unwrap(), vec![7u8; 300_000]);
        assert_eq!(fs::read(path.join("Audio/Takes/b.wav")).unwrap().len(), 1000);
        // A folder's own record travels along; temp files and the keep mark don't.
        assert!(path.join(".reamesh-link.json").is_file());
        assert!(!path.join(".reamesh-keep").exists());
        assert!(!path.join(".reamesh-part-1234").exists());
        assert!(!path.join(".reamesh-nofx.rpp").exists());
        assert_eq!(t.out_names(), ["Song"]);
    }

    #[test]
    fn a_taken_name_is_never_overwritten() {
        let t = Tree::new();
        fs::create_dir(t.out.join("Song")).unwrap();
        fs::write(t.out.join("Song/mine.txt"), b"keep me").unwrap();
        // A file by that name counts as much as a folder does.
        fs::write(t.out.join("Song 2"), b"me too").unwrap();
        let path = t.copy().unwrap();
        assert_eq!(path, t.out.join("Song 3"));
        assert_eq!(fs::read(t.out.join("Song/mine.txt")).unwrap(), b"keep me");
        assert_eq!(fs::read(t.out.join("Song 2")).unwrap(), b"me too");
        assert!(!t.out.join("Song/Song.rpp").exists());
        assert_eq!(t.copy().unwrap(), t.out.join("Song 4"));
    }

    #[test]
    fn a_full_disk_sends_the_partial_copy_to_the_trash() {
        let t = Tree::new();
        let mut budget = 100_000usize;
        let mut discarded = Vec::new();
        let failed = t
            .run(
                &mut |f, b| {
                    if b.len() > budget {
                        return Err(ErrorKind::StorageFull.into());
                    }
                    budget -= b.len();
                    f.write_all(b)
                },
                &mut |p| {
                    discarded.push(p.to_path_buf());
                    true
                },
            )
            .unwrap_err();
        assert_eq!(failed, Failed { why: Why::NoSpace, partial: Partial::Trashed });
        assert_eq!(discarded, [t.out.join("Song.reamesh-incomplete")]);
        // The test's Trash takes nothing away, and nothing else was touched.
        assert_eq!(t.out_names(), ["Song.reamesh-incomplete"]);
        assert!(t.source.join("Audio/a.wav").is_file());
    }

    #[test]
    fn without_a_trash_the_partial_copy_keeps_its_incomplete_name() {
        let t = Tree::new();
        let failed = t
            .run(&mut |_, _| Err(io::Error::other("drive gone")), &mut |_| false)
            .unwrap_err();
        let kept = t.out.join("Song.reamesh-incomplete");
        assert_eq!(failed.why, Why::Io("drive gone".into()));
        assert_eq!(failed.partial, Partial::Kept(kept.clone()));
        assert!(kept.is_dir());
        assert!(!t.out.join("Song").exists());
        // The next attempt doesn't trip over the remains, and doesn't touch them.
        fs::write(kept.join("evidence"), b"x").unwrap();
        assert_eq!(t.copy().unwrap(), t.out.join("Song"));
        assert!(kept.join("evidence").is_file());
        assert_eq!(t.out_names(), ["Song", "Song.reamesh-incomplete"]);
    }

    #[test]
    fn no_room_is_found_before_anything_is_written() {
        let t = Tree::new();
        let failed = save_copy(
            &t.source,
            "Song",
            &t.out,
            &t.staging,
            Hooks {
                free: &|_| Some(COPY_RESERVE),
                write: &mut |_, _| panic!("nothing to write"),
                discard: &mut |_| panic!("nothing to discard"),
                progress: &mut |_, _| panic!("nothing to report"),
            },
        )
        .unwrap_err();
        assert_eq!(failed, Failed { why: Why::NoSpace, partial: Partial::None });
        assert!(t.out_names().is_empty());
    }

    #[test]
    fn progress_ends_at_the_total() {
        let t = Tree::new();
        let mut seen = Vec::new();
        save_copy(
            &t.source,
            "Song",
            &t.out,
            &t.staging,
            Hooks {
                free: &available_space,
                write: &mut |f, b| f.write_all(b),
                discard: &mut |_| false,
                progress: &mut |done, total| seen.push((done, total)),
            },
        )
        .unwrap();
        let total = 300_000 + 1000 + 18 + 2;
        assert_eq!(seen.first(), Some(&(0, total)));
        assert_eq!(seen.last(), Some(&(total, total)));
    }

    #[test]
    fn the_staging_root_is_no_place_for_a_copy() {
        let t = Tree::new();
        for dest in [t.staging.clone(), t.source.clone()] {
            let failed = save_copy(
                &t.source,
                "Song",
                &dest,
                &t.staging,
                Hooks {
                    free: &|_| None,
                    write: &mut |_, _| panic!("nothing to write"),
                    discard: &mut |_| panic!("nothing to discard"),
                    progress: &mut |_, _| panic!("nothing to report"),
                },
            )
            .unwrap_err();
            assert_eq!(failed, Failed { why: Why::InsideStaging, partial: Partial::None });
        }
    }

    #[test]
    fn a_folder_that_is_not_there_fails_without_a_trace() {
        let t = Tree::new();
        let missing = t.out.join("nope");
        let failed = save_copy(
            &t.source,
            "Song",
            &missing,
            &t.staging,
            Hooks {
                free: &|_| None,
                write: &mut |_, _| panic!("nothing to write"),
                discard: &mut |_| panic!("nothing to discard"),
                progress: &mut |_, _| panic!("nothing to report"),
            },
        )
        .unwrap_err();
        assert!(matches!(failed.why, Why::Io(_)));
        assert_eq!(failed.partial, Partial::None);
        assert!(!missing.exists());
    }

    #[cfg(windows)]
    #[test]
    fn a_new_file_held_open_only_delays_the_name() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = Tree::new();
        let incomplete = t.out.join("Song.reamesh-incomplete");
        fs::create_dir(&incomplete).unwrap();
        fs::write(incomplete.join("a.wav"), b"x").unwrap();
        // Read sharing only, like a scanner holding it: no renaming around it.
        let held = fs::OpenOptions::new().read(true).share_mode(1).open(incomplete.join("a.wav")).unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(700));
            drop(held);
        });
        assert_eq!(publish(&incomplete, &t.out, "Song").unwrap(), t.out.join("Song"));
        release.join().unwrap();
        assert!(t.out.join("Song/a.wav").is_file());
    }

    #[test]
    fn only_one_copy_at_a_time() {
        let slot = CopySlot::take().unwrap();
        assert!(CopySlot::take().is_none());
        drop(slot);
        assert!(CopySlot::take().is_some());
    }

    #[test]
    fn the_app_hears_what_happened() {
        let ok = finished("Song-0f3a".into(), Ok(PathBuf::from("/out/Song")));
        assert_eq!(ok.path.as_deref(), Some("/out/Song"));
        assert!(ok.error.is_none() && !ok.trashed && ok.kept.is_none());
        let full = finished(
            "Song-0f3a".into(),
            Err(Failed { why: Why::NoSpace, partial: Partial::Trashed }),
        );
        assert_eq!(full.error.as_deref(), Some("no_space"));
        assert!(full.path.is_none() && full.trashed);
        let gone = finished(
            "Song-0f3a".into(),
            Err(Failed { why: Why::Io("drive gone".into()), partial: Partial::Kept("/out/x".into()) }),
        );
        assert_eq!(gone.error.as_deref(), Some("drive gone"));
        assert_eq!(gone.kept.as_deref(), Some("/out/x"));
    }
}
