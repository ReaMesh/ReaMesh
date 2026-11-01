//! `seen.json`: when a host last held a reservation with each key, for the
//! LAST SEEN column of `key list`. The running relay keeps it; a key that is
//! removed from config.toml drops out of it with the next save.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::SEEN_FILE;

#[derive(Debug, Default)]
pub struct Seen {
    /// Key label → Unix seconds.
    pub last: BTreeMap<String, u64>,
    path: Option<PathBuf>,
    dirty: bool,
}

impl Seen {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(SEEN_FILE);
        let last = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            last,
            path: Some(path),
            dirty: false,
        }
    }

    pub fn touch(&mut self, label: &str, now: u64) {
        let t = self.last.entry(label.to_owned()).or_default();
        if *t < now {
            *t = now;
            self.dirty = true;
        }
    }

    /// Writes the file when anything changed, keeping only `labels`.
    pub fn save<'a>(&mut self, labels: impl IntoIterator<Item = &'a str>) -> std::io::Result<()> {
        let keep: std::collections::HashSet<&str> = labels.into_iter().collect();
        let before = self.last.len();
        self.last.retain(|l, _| keep.contains(l.as_str()));
        self.dirty |= self.last.len() != before;
        let Some(path) = &self.path else {
            return Ok(());
        };
        if !self.dirty {
            return Ok(());
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.last)?)?;
        std::fs::rename(tmp, path)?;
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_newest_time_and_drops_removed_keys() {
        let dir = tempfile::tempdir().unwrap();
        let mut seen = Seen::load(dir.path());
        seen.touch("band", 100);
        seen.touch("band", 50);
        seen.touch("lena", 70);
        seen.save(["band", "lena"]).unwrap();
        let mut again = Seen::load(dir.path());
        assert_eq!(again.last.get("band"), Some(&100));
        again.save(["band"]).unwrap();
        let last = Seen::load(dir.path()).last;
        assert_eq!(last.len(), 1);
        assert_eq!(last.get("band"), Some(&100));
    }
}
