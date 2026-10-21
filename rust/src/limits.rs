//! Bounds on what peers can make this node store or do: the shape and size of
//! proposed fields, the freshness of the owner's control messages, the size of
//! lobby/presence data kept per peer, and how fast address hints are dialed.
//!
//! Everything here is pure (or a small self-contained piece of state) so it
//! can be unit-tested without a swarm.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Map as JsonMap, Value as JsonValue};

use super::{ITEMS_ROOT, PROJECT_ROOT, TRACKS_ROOT};

// ---------- proposal shape ----------

/// Largest single chunk-carried field (an FX chain with plugin state, an item
/// chunk, an envelope set). Generous: some plugins keep samples in their
/// state. A whole proposal is additionally bounded by the gossip message size.
pub(crate) const MAX_CHUNK_FIELD: usize = 32 * 1024 * 1024;
/// Most entities one proposal may touch per section.
pub(crate) const MAX_ENTITIES: usize = 100_000;
/// Longest entity id (REAPER GUIDs are 38 characters).
const MAX_ID_LEN: usize = 128;

#[derive(Clone, Copy)]
enum Spec {
    Bool,
    Num,
    /// A string of at most this many bytes.
    Str(usize),
    /// A track chunk part (see `valid_track_part`), at most this many bytes.
    Part(usize),
    /// An item chunk (see `valid_item_chunk`).
    Item,
}

/// The fields capture produces, per section. Anything else in a proposal is
/// dropped: the doc only ever holds what the extension knows how to apply.
fn field_spec(section: &str, key: &str) -> Option<Spec> {
    use Spec::*;
    match section {
        TRACKS_ROOT => Some(match key {
            "present" | "fxen" | "phase" | "solodefeat" | "mainsend" => Bool,
            "index" | "vol" | "pan" | "width" | "panmode" | "dualpanl" | "dualpanr" | "panlaw"
            | "panlawflags" | "nchan" | "color" | "folderdepth" | "mainsendoffs" | "mainsendnch"
            | "timebase" | "playoffs" | "playoffsflag" | "freemode" | "numlanes"
            | "lanesettings" => Num,
            "name" => Str(4 * 1024),
            "lanenames" => Str(512 * 1024),
            "laneplays" => Str(4 * 1024),
            "fxchain" | "recfx" | "env" | "recv" => Part(MAX_CHUNK_FIELD),
            "group" | "lock" => Part(4 * 1024),
            _ => return None,
        }),
        ITEMS_ROOT => Some(match key {
            "present" => Bool,
            "track" => Str(MAX_ID_LEN),
            "chunk" => Item,
            _ => return None,
        }),
        PROJECT_ROOT => Some(match key {
            "tempo" | "markers" => Str(4 * 1024 * 1024),
            "notes" => Str(1024 * 1024),
            "title" | "author" => Str(4 * 1024),
            "groupnames" => Str(512 * 1024),
            "timebase" => Str(64),
            _ => return None,
        }),
        _ => None,
    }
}

fn ltrim(s: &str) -> &str {
    s.trim_start_matches([' ', '\t'])
}

/// The tag of a block-opening line (`<FXCHAIN ...` -> `FXCHAIN`).
fn block_tag(t: &str) -> Option<&str> {
    t.strip_prefix('<').map(|r| r.split([' ', '\t']).next().unwrap_or(""))
}

/// `KEYWORD int int ...` with at least one integer; `keyword_ok` judges the
/// keyword.
fn keyword_ints(t: &str, keyword_ok: impl Fn(&str) -> bool) -> bool {
    let mut tok = t.split([' ', '\t']).filter(|s| !s.is_empty());
    let Some(kw) = tok.next() else { return false };
    if !keyword_ok(kw) {
        return false;
    }
    let mut n = 0;
    for v in tok {
        let digits = v.strip_prefix('-').unwrap_or(v);
        if digits.is_empty() || digits.len() > 20 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        n += 1;
    }
    n > 0
}

/// Whether `text` is a well-formed track chunk part `field`, in the shape the
/// extension's capture produces (mirrors `validTrackPart` in reamesh.cpp, which
/// re-checks everything on apply): LF-only, '\n'-terminated, balanced blocks
/// that never close the enclosing track; one `LOCK <ints>` line;
/// `GROUP_FLAGS*` integer lines; receives = AUXRECV lines plus AUX*ENV
/// send-envelope blocks, starting with an AUXRECV line; envelopes = *ENV*
/// blocks; FX chains = exactly one <FXCHAIN / <FXCHAIN_REC block. Envelope and
/// send-envelope blocks carry no nested block but extension data (<EXT, one
/// level deep). Empty is valid (removes the part).
pub(crate) fn valid_track_part(field: &str, text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    if !text.ends_with('\n') || text.contains('\r') || text.contains('\0') {
        return false;
    }
    let mut depth = 1i32;
    let (mut blocks_top, mut nested) = (0usize, false);
    let mut top_lines: Vec<&str> = Vec::new();
    let mut top_tags: Vec<&str> = Vec::new();
    let mut first = "";
    for (i, line) in text[..text.len() - 1].split('\n').enumerate() {
        let t = ltrim(line);
        if i == 0 {
            first = t;
        }
        if let Some(tag) = block_tag(t) {
            if depth == 1 {
                blocks_top += 1;
                top_tags.push(tag);
            } else if depth != 2 || tag != "EXT" {
                // The one nested block an envelope carries: <EXT (extension
                // key/value data), directly inside it.
                nested = true;
            }
            depth += 1;
        } else if t == ">" {
            depth -= 1;
            if depth < 1 {
                return false;
            }
        } else if depth == 1 {
            top_lines.push(t);
        }
    }
    if depth != 1 {
        return false;
    }
    let is_env = |tag: &&str| tag.contains("ENV");
    match field {
        "lock" => blocks_top == 0 && top_lines.len() == 1 && keyword_ints(first, |k| k == "LOCK"),
        "group" => {
            blocks_top == 0
                && top_lines.len() <= 8
                && top_lines.iter().all(|t| {
                    keyword_ints(t, |k| {
                        k.starts_with("GROUP_FLAGS")
                            && k.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
                    })
                })
        }
        "recv" => {
            !nested
                && first.starts_with("AUXRECV ")
                && top_lines.iter().all(|t| t.starts_with("AUXRECV "))
                && top_tags.iter().all(|t| is_env(t) && t.starts_with("AUX"))
        }
        "env" => {
            !nested
                && top_lines.is_empty()
                && top_tags.iter().all(|t| is_env(t) && !t.starts_with("AUX"))
        }
        "fxchain" => blocks_top == 1 && top_lines.is_empty() && top_tags == ["FXCHAIN"],
        "recfx" => blocks_top == 1 && top_lines.is_empty() && top_tags == ["FXCHAIN_REC"],
        _ => false,
    }
}

/// Whether `chunk` is one balanced <ITEM block and nothing else (a trailing
/// newline aside). Mirrors `validItemChunk` in reamesh.cpp.
pub(crate) fn valid_item_chunk(chunk: &str) -> bool {
    if chunk.contains('\0') {
        return false;
    }
    let (mut depth, mut first, mut closed) = (0i32, true, false);
    for line in chunk.split('\n') {
        let t = ltrim(line.strip_suffix('\r').unwrap_or(line));
        if closed {
            if !t.is_empty() {
                return false;
            }
            continue;
        }
        if first {
            if block_tag(t) != Some("ITEM") {
                return false;
            }
            first = false;
            depth = 1;
            continue;
        }
        if t.starts_with('<') {
            depth += 1;
        } else if t == ">" {
            depth -= 1;
            if depth == 0 {
                closed = true;
            }
        }
    }
    closed
}

fn spec_ok(spec: Spec, key: &str, value: &JsonValue) -> bool {
    match spec {
        Spec::Bool => value.is_boolean(),
        Spec::Num => value.as_f64().is_some_and(f64::is_finite),
        Spec::Str(max) => value.as_str().is_some_and(|s| s.len() <= max),
        Spec::Part(max) => value
            .as_str()
            .is_some_and(|s| s.len() <= max && valid_track_part(key, s)),
        Spec::Item => value
            .as_str()
            .is_some_and(|s| s.len() <= MAX_CHUNK_FIELD && valid_item_chunk(s)),
    }
}

/// Bring a client's proposed changeset into the shape the doc may hold:
/// known sections and fields only, the right JSON type, bounded sizes,
/// well-formed chunk parts and item chunks, bounded ids and entity counts, the
/// single `project` id for the project section; and no marker list that brings
/// in a new action marker (see `adds_action_marker`). `current_markers` is the
/// doc's marker list now. Returns how many fields/entities were dropped.
pub(crate) fn sanitize_proposal(cs: &mut JsonValue, current_markers: Option<&str>) -> usize {
    let Some(obj) = cs.as_object_mut() else {
        *cs = JsonValue::Object(JsonMap::new());
        return 1;
    };
    let mut dropped = 0;
    let before = obj.len();
    obj.retain(|k, _| matches!(k.as_str(), TRACKS_ROOT | ITEMS_ROOT | PROJECT_ROOT));
    dropped += before - obj.len();
    for (section, entities) in obj.iter_mut() {
        let Some(entities) = entities.as_object_mut() else {
            *entities = JsonValue::Object(JsonMap::new());
            dropped += 1;
            continue;
        };
        let mut kept = 0usize;
        let before = entities.len();
        entities.retain(|id, fields| {
            let ok = !id.is_empty()
                && id.len() <= MAX_ID_LEN
                && (section != PROJECT_ROOT || id == "project")
                && fields.is_object()
                && kept < MAX_ENTITIES;
            if ok {
                kept += 1;
            }
            ok
        });
        dropped += before - entities.len();
        for fields in entities.values_mut() {
            let Some(fields) = fields.as_object_mut() else { continue };
            let before = fields.len();
            fields.retain(|key, value| {
                field_spec(section, key).is_some_and(|spec| spec_ok(spec, key, value))
            });
            dropped += before - fields.len();
            if section == PROJECT_ROOT
                && let Some(m) = fields.get("markers").and_then(|v| v.as_str())
                && adds_action_marker(m, current_markers.unwrap_or(""))
            {
                fields.remove("markers");
                dropped += 1;
            }
        }
        entities.retain(|_, f| f.as_object().is_some_and(|f| !f.is_empty()));
    }
    obj.retain(|_, v| v.as_object().is_some_and(|e| !e.is_empty()));
    dropped
}

// ---------- marker actions ----------

/// A marker/region name REAPER would run as an action (`!<command id>`, with
/// marker actions enabled). Mirrors `isActionMarkerName` in reamesh.cpp.
fn is_action_marker_name(name: &str) -> bool {
    name.trim_start_matches(' ').starts_with('!')
}

/// The marker/region names in a marker list: whatever follows the sixth tab
/// of a `M|R\tnum\tpos\tend\tcolor\tlane\tname` line, exactly where
/// `parseMarkerList` in reamesh.cpp reads the name. `L\t…` lines are ruler
/// lanes, whose names REAPER never runs; every other line counts, so a line
/// the extension would reject can't hide a name from this check either.
fn marker_names(markers: &str) -> impl Iterator<Item = &str> {
    markers
        .split('\n')
        .filter(|line| !line.starts_with("L\t"))
        .filter_map(|line| line.splitn(7, '\t').nth(6))
}

/// Whether the proposed marker list brings in an action-like name that isn't
/// in the current list already. A guest may keep or move the host's action
/// markers but never create or rename one: whatever it adds would run as an
/// action on the host's machine. (Clients additionally never apply an action
/// name verbatim; see the marker actions note in reamesh.cpp.)
pub(crate) fn adds_action_marker(proposed: &str, current: &str) -> bool {
    let existing: HashSet<&str> =
        marker_names(current).filter(|n| is_action_marker_name(n)).collect();
    marker_names(proposed).any(|n| is_action_marker_name(n) && !existing.contains(n))
}

// ---------- owner control messages: freshness ----------

/// Highest owner control-message sequence accepted, per stream. Sealed
/// session messages carry no freshness of their own, and gossipsub forgets a
/// message id after a couple of minutes, so without this an old 'K' (kick),
/// 'B' (ban table) or 'L' (lock table) could be re-forwarded by any topic
/// member and acted on again. Process-global so it survives a rejoin (a new
/// session handle) within the same REAPER. Keyed by `ctl_stream`.
static CTL_SEEN: std::sync::LazyLock<Mutex<HashMap<String, u64>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
const CTL_SEEN_MAX: usize = 4096;

/// The freshness stream of one control message: who receives it, on which
/// session topic, from which owner, and which kind (a kick notice per target,
/// since only the newest notice about one peer matters).
pub(crate) fn ctl_stream(me: &str, topic: &str, owner: &str, kind: &str) -> String {
    format!("{me}|{topic}|{owner}|{kind}")
}

/// Accept `seq` on `stream` only if it is newer than everything accepted
/// there before. Owners stamp a strictly increasing, wall-clock-anchored
/// sequence (see `Reamesh::next_ctl_seq`), so a re-host keeps counting up and a
/// message without one (seq 0) never passes.
pub(crate) fn ctl_fresh(stream: String, seq: u64) -> bool {
    let mut seen = CTL_SEEN.lock().unwrap();
    if seq <= seen.get(&stream).copied().unwrap_or(0) {
        return false;
    }
    if seen.len() >= CTL_SEEN_MAX && !seen.contains_key(&stream) {
        // Bounded: streams come only from owners this process joined.
        seen.clear();
    }
    seen.insert(stream, seq);
    true
}

/// A strictly increasing sequence anchored to wall-clock microseconds:
/// `prev` is the last one handed out (an atomic in the caller's state).
pub(crate) fn next_anchored_seq(prev: &std::sync::atomic::AtomicU64) -> u64 {
    use std::sync::atomic::Ordering;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    let mut cur = prev.load(Ordering::Relaxed);
    loop {
        let next = now.max(cur + 1);
        match prev.compare_exchange_weak(cur, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(actual) => cur = actual,
        }
    }
}

// ---------- lobby / presence data ----------

/// Largest lobby message parsed at all (an announce is a few hundred bytes).
pub(crate) const ANNOUNCE_MAX_BYTES: usize = 8 * 1024;
/// Largest presence beacon / ping parsed at all (a beacon with the full
/// 256-entry selection is ~12 KB).
pub(crate) const BEACON_MAX_BYTES: usize = 64 * 1024;
/// Most announced sessions kept; the stalest go first.
pub(crate) const MAX_SESSIONS: usize = 64;
/// Most roster entries kept; the stalest go first.
pub(crate) const MAX_PEERS_INFO: usize = 128;

/// At most `max` characters of `s`.
pub(crate) fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// A plausible announced salt: hex, 16..=128 characters (hosts send 32).
pub(crate) fn valid_salt(s: &str) -> bool {
    (16..=128).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Make room for one new entry in a map that holds at most `max`: evict the
/// entry `age` calls oldest until there is space. No-op when `key` is
/// already present.
pub(crate) fn evict_for<V>(
    map: &mut HashMap<String, V>,
    key: &str,
    max: usize,
    age: impl Fn(&V) -> Instant,
) {
    if map.contains_key(key) {
        return;
    }
    while map.len() >= max {
        let Some(oldest) = map.iter().min_by_key(|(_, v)| age(v)).map(|(k, _)| k.clone()) else {
            break;
        };
        map.remove(&oldest);
    }
}

// ---------- address hints ----------

/// Most hint dials per `HINT_RATE_WINDOW`, across all hints together.
pub(crate) const HINT_RATE_MAX: usize = 32;
pub(crate) const HINT_RATE_WINDOW: Duration = Duration::from_secs(60);
/// Most peers that came in through a hint and are kept as explicit gossipsub
/// peers at once.
pub(crate) const HINT_PEERS_MAX: usize = 16;

/// Global rate cap for hint dials (each address has its own re-dial gap on
/// top; this bounds what a stream of fresh addresses can make us dial).
#[derive(Default)]
pub(crate) struct HintRate {
    dials: VecDeque<Instant>,
}

impl HintRate {
    /// Whether one more dial is allowed now; counts it if so.
    pub(crate) fn allow(&mut self, now: Instant) -> bool {
        while self.dials.front().is_some_and(|t| now.duration_since(*t) >= HINT_RATE_WINDOW) {
            self.dials.pop_front();
        }
        if self.dials.len() >= HINT_RATE_MAX {
            return false;
        }
        self.dials.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TRACK_PARTS: &[(&str, &str)] = &[
        ("lock", "  LOCK 1\n"),
        ("group", "  GROUP_FLAGS 1 0 0\n  GROUP_FLAGS_HIGH 0 0 1\n"),
        (
            "recv",
            "  AUXRECV {G1} 0 1 0 0 0 0 0 0 -1 0 -1 ''\n  <AUXVOLENV\n    ACT 1 -1\n    PT 0 1 0\n  >\n  AUXRECV {G2} 0 0.5\n",
        ),
        ("env", "  <VOLENV2\n    ACT 1 -1\n    PT 0 1 0\n  >\n  <PANENV2\n    PT 0 0 0\n  >\n"),
        ("fxchain", "  <FXCHAIN\n    BYPASS 0 0 0\n    <VST \"VST: ReaEQ\" reaeq.dylib 0\n      ZGF0YQ==\n    >\n  >\n"),
        ("recfx", "  <FXCHAIN_REC\n    <JS gate\n    >\n  >\n"),
    ];

    #[test]
    fn captured_track_parts_are_valid() {
        for (field, text) in TRACK_PARTS {
            assert!(valid_track_part(field, text), "{field}");
            assert!(valid_track_part(field, ""), "{field} empty");
        }
        // Extension data inside an envelope is envelope content.
        assert!(valid_track_part("env", "  <VOLENV2\n    <EXT\n      key value\n    >\n    PT 0 1 0\n  >\n"));
        assert!(!valid_track_part("env", "  <VOLENV2\n    <EXT\n      <ITEM\n      >\n    >\n  >\n"));
        // Only as their own kind.
        assert!(!valid_track_part("env", TRACK_PARTS[4].1));
        assert!(!valid_track_part("lock", TRACK_PARTS[1].1));
        assert!(!valid_track_part("recv", TRACK_PARTS[3].1));
        assert!(!valid_track_part("nonsense", "LOCK 1\n"));
    }

    #[test]
    fn malformed_track_parts_are_refused() {
        let bad: &[(&str, &str)] = &[
            ("lock", "  LOCK 1\n  <FXCHAIN\n  >\n"),
            ("lock", "LOCK 1\nLOCK 1\n"),
            ("lock", "LOCK one\n"),
            ("lock", "LOCK 1"),
            ("lock", "LOCK 1\r\n"),
            ("lock", "LOCK 1\n>\n<FXCHAIN\n>\n"),
            ("group", "GROUP_FLAGS 1\n<FXCHAIN\n>\n"),
            ("group", "GROUP_FLAGSx 1\n"),
            ("env", "  <VOLENV2\n  >\n  <ITEM\n  >\n"),
            ("env", "  <VOLENV2\n    <SOURCE WAVE\n    >\n  >\n"),
            ("env", "  <VOLENV2\n    PT 0 1 0\n"),
            ("env", "  NAME x\n  <VOLENV2\n  >\n"),
            ("recv", "  NAME x\n  AUXRECV {G} 0\n"),
            ("recv", "  AUXRECV {G} 0\n  <FXCHAIN\n  >\n"),
            ("recv", "  AUXRECV {G} 0\n  <AUXVOLENV\n    <VST x\n    >\n  >\n"),
            ("fxchain", "  <FXCHAIN\n  >\n  <FXCHAIN\n  >\n"),
            ("fxchain", "  <FXCHAIN\n  >\n  NAME x\n"),
            ("fxchain", "  <FXCHAIN_REC\n  >\n"),
            ("recfx", "  <FXCHAIN\n  >\n"),
        ];
        for (field, text) in bad {
            assert!(!valid_track_part(field, text), "{field}: {text:?}");
        }
    }

    #[test]
    fn item_chunks_are_one_balanced_item() {
        assert!(valid_item_chunk("<ITEM\nPOSITION 2.0\n>"));
        assert!(valid_item_chunk("<ITEM\n  <SOURCE WAVE\n    FILE \"a.wav\"\n  >\n>\n"));
        assert!(!valid_item_chunk(""));
        assert!(!valid_item_chunk("<TRACK\n>\n"));
        assert!(!valid_item_chunk("<ITEM\n  POSITION 1\n"));
        assert!(!valid_item_chunk("<ITEM\n>\n<FXCHAIN\n>\n"));
        assert!(!valid_item_chunk("POSITION 1\n<ITEM\n>\n"));
    }

    #[test]
    fn proposals_keep_only_known_well_formed_fields() {
        let mut cs = json!({
            "tracks": {
                "{T1}": {
                    "name": "Bass", "vol": 0.5, "present": true,
                    "lock": "  LOCK 1\n  <FXCHAIN\n  >\n",
                    "env": TRACK_PARTS[3].1,
                    "evil": "x", "nchan": "two"
                },
                "{T2}": { "mute": true },
                "": { "name": "no id" }
            },
            "items": {
                "{I1}": { "chunk": "<ITEM\nPOSITION 1\n>\n", "track": "{T1}" },
                "{I2}": { "chunk": "<ITEM\n>\n<ITEM\n>\n" }
            },
            "project": {
                "project": { "title": "Song", "bogus": 1 },
                "other": { "title": "x" }
            },
            "extra": { "a": { "b": 1 } }
        });
        let dropped = sanitize_proposal(&mut cs, None);
        assert_eq!(
            cs,
            json!({
                "tracks": { "{T1}": { "name": "Bass", "vol": 0.5, "present": true, "env": TRACK_PARTS[3].1 } },
                "items": { "{I1}": { "chunk": "<ITEM\nPOSITION 1\n>\n", "track": "{T1}" } },
                "project": { "project": { "title": "Song" } }
            })
        );
        assert!(dropped >= 9, "{dropped}");

        // Size caps.
        let mut big = json!({ "tracks": { "{T}": { "name": "x".repeat(5000) } } });
        sanitize_proposal(&mut big, None);
        assert_eq!(big, json!({}));
        let mut huge = json!({ "items": { "{I}": { "chunk": format!("<ITEM\n{}\n>\n", "A".repeat(MAX_CHUNK_FIELD)) } } });
        sanitize_proposal(&mut huge, None);
        assert_eq!(huge, json!({}));

        // Not an object at all.
        let mut junk = json!([1, 2]);
        assert_eq!(sanitize_proposal(&mut junk, None), 1);
        assert_eq!(junk, json!({}));
    }

    #[test]
    fn guests_cannot_add_action_markers() {
        let current = "L\t3\t0\t0\t-1\t\nM\t1\t0\t0\t0\t0\t!1013\nM\t2\t4\t0\t0\t-1\tVerse\n";
        // Keeping or moving the host's own action marker is fine, onto
        // another lane too...
        assert!(!adds_action_marker(
            "L\t3\t0\t0\t-1\t\nL\t0\t0\t0\t-1\tFX\nM\t1\t8\t0\t0\t1\t!1013\nM\t2\t4\t0\t0\t-1\tVerse\n",
            current
        ));
        // ...adding or renaming one is not, regions included.
        assert!(adds_action_marker("M\t1\t0\t0\t0\t0\t!40044\n", current));
        assert!(adds_action_marker(&format!("{current}R\t3\t1\t2\t0\t0\t !40044\n"), current));
        assert!(!adds_action_marker("M\t1\t0\t0\t0\t0\tChorus !\n", ""));
        // A lane name is no action; a lane field can't hide a marker's name.
        assert!(!adds_action_marker("L\t0\t0\t0\t-1\t!40044\n", ""));
        assert!(adds_action_marker("M\t1\t0\t0\t0\tx\t!40044\n", ""));
        assert!(adds_action_marker("X\t1\t0\t0\t0\t0\t!40044\n", ""));

        let mut cs = json!({ "project": { "project": {
            "markers": "M\t1\t0\t0\t0\t0\t!40044\n", "title": "T" } } });
        assert_eq!(sanitize_proposal(&mut cs, Some(current)), 1);
        assert_eq!(cs, json!({ "project": { "project": { "title": "T" } } }));
        let mut ok = json!({ "project": { "project": { "markers": current } } });
        assert_eq!(sanitize_proposal(&mut ok, Some(current)), 0);
    }

    #[test]
    fn control_messages_must_be_newer_per_stream() {
        let s = |kind: &str| ctl_stream("me-test", "topic-test", "owner-test", kind);
        assert!(ctl_fresh(s("L"), 10));
        assert!(!ctl_fresh(s("L"), 10)); // replay
        assert!(!ctl_fresh(s("L"), 9)); // older
        assert!(ctl_fresh(s("L"), 11));
        // Streams are independent: an older ban table is judged on its own.
        assert!(ctl_fresh(s("B"), 5));
        assert!(ctl_fresh(s("K:a"), 3));
        assert!(ctl_fresh(s("K:b"), 2));
        assert!(!ctl_fresh(s("K:a"), 3));
        // No sequence at all never passes.
        assert!(!ctl_fresh(s("X"), 0));
        // Another receiver keeps its own floor.
        assert!(ctl_fresh(ctl_stream("other", "topic-test", "owner-test", "L"), 10));
    }

    #[test]
    fn anchored_sequence_strictly_increases() {
        let a = std::sync::atomic::AtomicU64::new(0);
        let mut last = 0;
        for _ in 0..1000 {
            let n = next_anchored_seq(&a);
            assert!(n > last);
            last = n;
        }
        // A counter ahead of the clock keeps counting up from itself.
        let ahead = std::sync::atomic::AtomicU64::new(u64::MAX / 2);
        assert_eq!(next_anchored_seq(&ahead), u64::MAX / 2 + 1);
    }

    #[test]
    fn clip_salt_and_eviction() {
        assert_eq!(clip("Grüße", 3), "Grü");
        assert!(valid_salt(&"ab".repeat(16)));
        assert!(!valid_salt("abc"));
        assert!(!valid_salt(&"zz".repeat(16)));
        assert!(!valid_salt(&"a".repeat(200)));

        let now = Instant::now();
        let mut m: HashMap<String, Instant> = HashMap::new();
        for i in 0..4u64 {
            m.insert(format!("k{i}"), now + Duration::from_secs(i));
        }
        evict_for(&mut m, "k9", 4, |v| *v);
        assert_eq!(m.len(), 3);
        assert!(!m.contains_key("k0")); // the stalest went
        evict_for(&mut m, "k1", 3, |v| *v); // present: nothing to do
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn hint_dials_are_rate_capped() {
        let mut r = HintRate::default();
        let t0 = Instant::now();
        for _ in 0..HINT_RATE_MAX {
            assert!(r.allow(t0));
        }
        assert!(!r.allow(t0 + Duration::from_secs(30)));
        assert!(r.allow(t0 + HINT_RATE_WINDOW));
    }
}
