//! The files Setup installs ride at the end of its own executable:
//!
//! ```text
//! [setup binary][blob][blake3(blob): 32][blob length: u64 LE][MAGIC: 8]
//! ```
//!
//! `blob` is one zstd stream of `[header length: u32 LE][header JSON][file
//! bytes, in header order]`. The binary without the tail is a complete program
//! of its own: it is what gets installed as `uninstall.exe`, and with no
//! payload Setup starts as the uninstaller.
//!
//! The tail is appended after the build (`src/bin/pack.rs`), so the setup
//! binary is compiled once and never sees the files it carries.

use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"RMSETUP\x01";
const TRAILER: u64 = 32 + 8 + 8;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Entry {
    /// File name inside the install folder (no folders).
    pub path: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Header {
    /// The ReaMesh version these files are.
    pub version: String,
    pub files: Vec<Entry>,
}

impl Header {
    pub fn total(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// Where the payload sits inside an executable.
#[derive(Clone, Debug)]
pub struct Located {
    /// Length of the bare setup binary, i.e. where the blob starts.
    pub stub_len: u64,
    pub blob_len: u64,
    pub hash: [u8; 32],
}

/// Finds the payload of `exe`; `None` for a bare binary (the uninstaller).
pub fn locate(exe: &Path) -> io::Result<Option<Located>> {
    let mut f = File::open(exe)?;
    let len = f.metadata()?.len();
    if len < TRAILER {
        return Ok(None);
    }
    f.seek(SeekFrom::Start(len - TRAILER))?;
    let mut tail = [0u8; TRAILER as usize];
    f.read_exact(&mut tail)?;
    if &tail[40..48] != MAGIC {
        return Ok(None);
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&tail[..32]);
    let blob_len = u64::from_le_bytes(tail[32..40].try_into().unwrap());
    if blob_len > len - TRAILER {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "damaged setup file"));
    }
    Ok(Some(Located { stub_len: len - TRAILER - blob_len, blob_len, hash }))
}

/// The blob, checked against its hash: a download cut short or altered on the
/// way never gets as far as the install folder.
pub fn read_blob(exe: &Path, at: &Located) -> io::Result<Vec<u8>> {
    let mut f = File::open(exe)?;
    f.seek(SeekFrom::Start(at.stub_len))?;
    let mut blob = vec![0u8; at.blob_len as usize];
    f.read_exact(&mut blob)?;
    if blake3::hash(&blob).as_bytes() != &at.hash {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "this setup file is damaged, download it again",
        ));
    }
    Ok(blob)
}

/// The header alone (decompresses only the start of the stream).
pub fn read_header(blob: &[u8]) -> io::Result<Header> {
    let mut dec = zstd::stream::read::Decoder::new(blob)?;
    header_from(&mut dec)
}

fn header_from(r: &mut impl Read) -> io::Result<Header> {
    let mut n = [0u8; 4];
    r.read_exact(&mut n)?;
    let n = u32::from_le_bytes(n) as usize;
    if n > 1 << 20 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "damaged setup file"));
    }
    let mut json = vec![0u8; n];
    r.read_exact(&mut json)?;
    let h: Header = serde_json::from_slice(&json)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    for f in &h.files {
        if !safe_name(&f.path) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "damaged setup file"));
        }
    }
    Ok(h)
}

/// A plain file name: what the install folder may receive.
pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < 128
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Streams every file out of the blob: `open(entry)` gives the writer for it,
/// `progress(bytes)` is called as data lands.
pub fn extract(
    blob: &[u8],
    mut open: impl FnMut(&Entry) -> io::Result<Box<dyn Write>>,
    mut progress: impl FnMut(u64),
) -> io::Result<Header> {
    let mut dec = zstd::stream::read::Decoder::new(blob)?;
    let header = header_from(&mut dec)?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    for entry in &header.files {
        let mut out = open(entry)?;
        let mut left = entry.size;
        while left > 0 {
            let want = left.min(buf.len() as u64) as usize;
            dec.read_exact(&mut buf[..want])?;
            out.write_all(&buf[..want])?;
            left -= want as u64;
            done += want as u64;
            progress(done);
        }
        out.flush()?;
    }
    Ok(header)
}

/// Builds the tail for `files` (name, bytes) and appends it to `stub`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn pack(stub: &[u8], version: &str, files: &[(String, Vec<u8>)], out: &mut impl Write) -> io::Result<()> {
    let header = Header {
        version: version.to_string(),
        files: files
            .iter()
            .map(|(p, b)| Entry { path: p.clone(), size: b.len() as u64 })
            .collect(),
    };
    for f in &header.files {
        if !safe_name(&f.path) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("bad file name {}", f.path)));
        }
    }
    let json = serde_json::to_vec(&header)?;
    let mut enc = zstd::stream::write::Encoder::new(Vec::new(), 19)?;
    enc.long_distance_matching(true)?;
    enc.include_checksum(true)?;
    enc.write_all(&(json.len() as u32).to_le_bytes())?;
    enc.write_all(&json)?;
    for (_, bytes) in files {
        enc.write_all(bytes)?;
    }
    let blob = enc.finish()?;
    out.write_all(stub)?;
    out.write_all(&blob)?;
    out.write_all(blake3::hash(&blob).as_bytes())?;
    out.write_all(&(blob.len() as u64).to_le_bytes())?;
    out.write_all(MAGIC)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("rmsetup-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("setup.exe");
        let files = vec![
            ("app.exe".to_string(), (0..300_000u32).map(|i| (i % 251) as u8).collect::<Vec<_>>()),
            ("reaper_reamesh.dll".to_string(), b"dll".to_vec()),
        ];
        let mut out = Vec::new();
        pack(b"MZstub", "9.9.9", &files, &mut out).unwrap();
        std::fs::write(&exe, &out).unwrap();

        let at = locate(&exe).unwrap().expect("payload");
        assert_eq!(at.stub_len, 6);
        let blob = read_blob(&exe, &at).unwrap();
        assert_eq!(read_header(&blob).unwrap().version, "9.9.9");

        let got = std::rc::Rc::new(std::cell::RefCell::new(Vec::<(String, Vec<u8>)>::new()));
        struct Sink(std::rc::Rc<std::cell::RefCell<Vec<(String, Vec<u8>)>>>);
        impl Write for Sink {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.0.borrow_mut().last_mut().unwrap().1.extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut last = 0;
        extract(
            &blob,
            |e| {
                got.borrow_mut().push((e.path.clone(), Vec::new()));
                Ok(Box::new(Sink(got.clone())))
            },
            |d| last = d,
        )
        .unwrap();
        assert_eq!(*got.borrow(), files);
        assert_eq!(last, 300_003);

        // A damaged blob is refused before anything is extracted.
        let mut bad = out.clone();
        bad[10] ^= 1;
        std::fs::write(&exe, &bad).unwrap();
        assert!(read_blob(&exe, &locate(&exe).unwrap().unwrap()).is_err());

        // The bare stub has no payload.
        std::fs::write(&exe, b"MZstub").unwrap();
        assert!(locate(&exe).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names() {
        assert!(safe_name("app.exe"));
        assert!(!safe_name("..\\x.exe"));
        assert!(!safe_name("a/b"));
        assert!(!safe_name(".hidden"));
        assert!(!safe_name(""));
    }
}
