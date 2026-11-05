// Linked copies: the folders "Save as" put a joined project in. Each carries its
// own identity (`.reamesh-link.json`, written by the core at every join and copied
// along); this keeps the list of where those folders are, one path per line in the
// app's config folder. A folder that isn't there right now (an unplugged drive) is
// left out of the listing but stays on the list.

use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use tauri::{AppHandle, Emitter, Manager};

const LINK_FILE: &str = ".reamesh-link.json";
/// A copy in the staging root that stays: the core's sweep passes over a folder
/// with this file. It is on the list as well, so it is linked like a saved copy.
const KEEP_FILE: &str = ".reamesh-keep";

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CopyLink {
    pub project: String,
    pub owner: String,
    pub project_file: String,
    pub name: String,
    pub hash: String,
}

#[derive(Debug, PartialEq, serde::Serialize)]
pub(crate) struct LinkedCopy {
    pub path: String,
    /// Kept where it is in the staging root, not saved to a folder of the user's.
    pub kept: bool,
    #[serde(flatten)]
    pub link: CopyLink,
}

/// A folder's own record of what it is a copy of, if it has a usable one.
pub(crate) fn read_link(dir: &Path) -> Option<CopyLink> {
    let link: CopyLink = serde_json::from_slice(&fs::read(dir.join(LINK_FILE)).ok()?).ok()?;
    (!link.project.is_empty() && !link.owner.is_empty()).then_some(link)
}

fn read_lines(list: &Path) -> Vec<String> {
    fs::read_to_string(list)
        .map(|s| s.lines().filter(|l| !l.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

fn write_lines(list: &Path, lines: &[String]) -> io::Result<()> {
    if let Some(parent) = list.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = list.with_extension("tmp");
    fs::write(&tmp, lines.iter().map(|l| format!("{l}\n")).collect::<String>())?;
    fs::rename(&tmp, list)
}

fn add_line(list: &Path, path: &str) -> io::Result<()> {
    let mut lines = read_lines(list);
    if !lines.iter().any(|l| l == path) {
        lines.push(path.to_string());
        write_lines(list, &lines)?;
    }
    Ok(())
}

fn remove_line(list: &Path, path: &str) -> io::Result<()> {
    let lines = read_lines(list);
    let kept: Vec<String> = lines.iter().filter(|l| *l != path).cloned().collect();
    if kept.len() != lines.len() {
        write_lines(list, &kept)?;
    }
    Ok(())
}

/// The folders on the list that are there and know what they are, the newest
/// link first: with two copies of a project, the one saved last is the one a join
/// goes into.
fn read_list(list: &Path) -> Vec<LinkedCopy> {
    let mut out: Vec<LinkedCopy> = read_lines(list)
        .into_iter()
        .filter_map(|path| {
            let dir = Path::new(&path);
            Some(LinkedCopy { link: read_link(dir)?, kept: is_kept(dir), path })
        })
        .collect();
    out.reverse();
    out
}

pub(crate) fn is_kept(dir: &Path) -> bool {
    dir.join(KEEP_FILE).exists()
}

/// Keep a copy in the staging root or stop keeping it. The marker is what the
/// sweep goes by, so it goes first and comes off last: a failure on the way leaves
/// a copy that stays. A copy without a record of what it is stays unlinked.
fn set_kept(list: &Path, dir: &Path, keep: bool) -> io::Result<()> {
    let path = dir.to_str().filter(|p| !p.contains('\n'));
    if keep {
        fs::write(dir.join(KEEP_FILE), b"")?;
        if let (Some(path), true) = (path, read_link(dir).is_some()) {
            add_line(list, path)?;
        }
        return Ok(());
    }
    if let Some(path) = path {
        remove_line(list, path)?;
    }
    match fs::remove_file(dir.join(KEEP_FILE)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// blake3 of a file, in the core's hex format. `None` when there is no such file.
fn hash_file(path: &Path) -> io::Result<Option<String>> {
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(Some(hasher.finalize().to_hex().to_string()))
}

/// The local project file of a linked copy. The name comes from a host's preview,
/// so nothing but a path inside the folder.
fn local_project_file(dir: &Path, project_file: &str) -> Option<PathBuf> {
    let rel = Path::new(project_file);
    (!project_file.is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_))))
        .then(|| dir.join(rel))
}

fn list_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_config_dir().map_err(|e| e.to_string())?.join("linked_projects"))
}

/// A finished "save as": the new folder is linked if it knows what it is a copy of.
/// (A copy from before the links has no record and just stays a copy.)
pub(crate) fn remember(app: &AppHandle, folder: &Path) -> bool {
    let Some(path) = folder.to_str().filter(|p| !p.contains('\n')) else { return false };
    if read_link(folder).is_none() {
        return false;
    }
    let added = list_file(app).and_then(|l| add_line(&l, path).map_err(|e| e.to_string()));
    match added {
        Ok(()) => {
            let _ = app.emit("linked-projects-changed", ());
            true
        }
        Err(e) => {
            eprintln!("couldn't remember the linked copy: {e}");
            false
        }
    }
}

#[tauri::command]
pub(crate) async fn linked_projects(app: AppHandle) -> Result<Vec<LinkedCopy>, String> {
    let list = list_file(&app)?;
    tauri::async_runtime::spawn_blocking(move || read_list(&list)).await.map_err(|e| e.to_string())
}

/// Take a folder off the list. Nothing on the disk changes.
#[tauri::command]
pub(crate) async fn unlink_project(app: AppHandle, path: String) -> Result<(), String> {
    let list = list_file(&app)?;
    remove_line(&list, &path).map_err(|e| e.to_string())?;
    let _ = app.emit("linked-projects-changed", ());
    Ok(())
}

/// See `set_kept`. `dir` was checked by the caller to lie in the staging root.
pub(crate) fn keep(app: &AppHandle, dir: &Path, keep: bool) -> Result<(), String> {
    set_kept(&list_file(app)?, dir, keep).map_err(|e| e.to_string())?;
    let _ = app.emit("linked-projects-changed", ());
    Ok(())
}

/// A folder that went to the Trash needs no line on the list any more.
pub(crate) fn forget(app: &AppHandle, dir: &Path) {
    let Some(path) = dir.to_str() else { return };
    if list_file(app).and_then(|l| remove_line(&l, path).map_err(|e| e.to_string())).is_ok() {
        let _ = app.emit("linked-projects-changed", ());
    }
}

/// The blake3 of a linked copy's project file, `null` when it has none by that name.
#[tauri::command]
pub(crate) async fn link_state(
    app: AppHandle,
    path: String,
    project_file: String,
) -> Result<Option<String>, String> {
    let list = list_file(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        if !read_lines(&list).contains(&path) {
            return Err("not a linked folder".to_string());
        }
        let Some(file) = local_project_file(Path::new(&path), &project_file) else {
            return Ok(None);
        };
        hash_file(&file).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let root = std::env::temp_dir().join(format!(
                "reamesh-links-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Scratch(root)
        }

        fn folder(&self, name: &str, project: &str) -> String {
            let dir = self.0.join(name);
            fs::create_dir_all(&dir).unwrap();
            let link = CopyLink {
                project: project.into(),
                owner: "12D3KooWAlex".into(),
                project_file: "Song.rpp".into(),
                name: name.into(),
                hash: "abc".into(),
            };
            fs::write(dir.join(LINK_FILE), serde_json::to_vec(&link).unwrap()).unwrap();
            dir.to_string_lossy().into_owned()
        }

        fn list(&self) -> PathBuf {
            self.0.join("config").join("linked_projects")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_missing_folder_is_left_out_but_stays_on_the_list() {
        let s = Scratch::new();
        let a = s.folder("A", "p1");
        let b = s.folder("B", "p2");
        add_line(&s.list(), &a).unwrap();
        add_line(&s.list(), &b).unwrap();
        add_line(&s.list(), &a).unwrap();
        assert_eq!(read_lines(&s.list()), [a.clone(), b.clone()], "no line twice");

        let paths = |l: Vec<LinkedCopy>| l.into_iter().map(|c| c.path).collect::<Vec<_>>();
        assert_eq!(paths(read_list(&s.list())), [b.clone(), a.clone()], "the newest first");

        // The drive is gone: not listed, not forgotten.
        fs::rename(&a, s.0.join("elsewhere")).unwrap();
        assert_eq!(paths(read_list(&s.list())), [b.clone()]);
        assert_eq!(read_lines(&s.list()), [a.clone(), b.clone()]);
        fs::rename(s.0.join("elsewhere"), &a).unwrap();
        assert_eq!(paths(read_list(&s.list())), [b, a]);
    }

    #[test]
    fn a_kept_copy_is_linked_and_marked_until_it_stops_being_kept() {
        let s = Scratch::new();
        let a = s.folder("A", "p1");
        let dir = Path::new(&a);
        set_kept(&s.list(), dir, true).unwrap();
        let listed = read_list(&s.list());
        assert_eq!(listed.len(), 1);
        assert!(listed[0].kept && is_kept(dir));

        set_kept(&s.list(), dir, false).unwrap();
        assert!(read_list(&s.list()).is_empty());
        assert!(!is_kept(dir));
        assert!(dir.join(LINK_FILE).exists(), "the copy's own record stays");
        set_kept(&s.list(), dir, false).expect("twice is fine");
    }

    #[test]
    fn a_copy_without_a_record_can_be_kept_but_not_linked() {
        let s = Scratch::new();
        let dir = s.0.join("Old");
        fs::create_dir_all(&dir).unwrap();
        set_kept(&s.list(), &dir, true).unwrap();
        assert!(is_kept(&dir));
        assert!(read_lines(&s.list()).is_empty());
    }

    #[test]
    fn a_saved_copy_is_not_a_kept_one() {
        let s = Scratch::new();
        let a = s.folder("A", "p1");
        add_line(&s.list(), &a).unwrap();
        assert!(!read_list(&s.list())[0].kept);
    }

    #[test]
    fn unlinking_touches_nothing_on_the_disk() {
        let s = Scratch::new();
        let a = s.folder("A", "p1");
        fs::write(Path::new(&a).join("Song.rpp"), b"<REAPER_PROJECT\n>\n").unwrap();
        add_line(&s.list(), &a).unwrap();
        remove_line(&s.list(), &a).unwrap();
        assert!(read_list(&s.list()).is_empty());
        assert!(Path::new(&a).join("Song.rpp").is_file());
        assert!(Path::new(&a).join(LINK_FILE).is_file());
        // A path that isn't listed is no error.
        remove_line(&s.list(), "elsewhere").unwrap();
    }

    #[test]
    fn a_folder_without_a_usable_record_is_no_link() {
        let s = Scratch::new();
        let dir = s.0.join("Old");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_link(&dir), None, "from before the links");
        fs::write(dir.join(LINK_FILE), b"not json").unwrap();
        assert_eq!(read_link(&dir), None);
        fs::write(dir.join(LINK_FILE), br#"{"project":"","owner":"x","project_file":"a","name":"a","hash":"a"}"#).unwrap();
        assert_eq!(read_link(&dir), None);
    }

    #[test]
    fn the_hash_is_the_cores() {
        let s = Scratch::new();
        let file = s.0.join("Song.rpp");
        fs::write(&file, b"abc").unwrap();
        assert_eq!(
            hash_file(&file).unwrap().as_deref(),
            Some("6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"),
        );
        assert_eq!(hash_file(&s.0.join("nope.rpp")).unwrap(), None);
    }

    #[test]
    fn a_hosts_file_name_stays_inside_the_folder() {
        let dir = Path::new("copy");
        assert_eq!(local_project_file(dir, "Song.rpp"), Some(dir.join("Song.rpp")));
        assert_eq!(local_project_file(dir, "sub/Song.rpp"), Some(dir.join("sub/Song.rpp")));
        for bad in ["", "../Song.rpp", "sub/../../Song.rpp", "/etc/passwd", "./Song.rpp"] {
            assert_eq!(local_project_file(dir, bad), None, "{bad}");
        }
    }
}
