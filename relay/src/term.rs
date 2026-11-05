//! Terminal output, in the app's palette: greys only, bold for what matters,
//! amber for the warnings an operator has to act on (`limit`, `budget`), red
//! only for the ✗ of an error. Colour and symbols go together and both drop
//! out when the stream isn't a terminal or `NO_COLOR` is set, so `docker logs`
//! and journald get plain text.
//!
//! Everything is written through `anstream`, which also turns the escapes into
//! console calls on an old Windows console.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anstream::ColorChoice;
use anstream::stream::RawStream;

// The ReaMesh R. Rendered from proprietary/logo/mark.svg; like the rest of
// the brand artwork it is not covered by the GPL.
const MARK: &str = include_str!("../../proprietary/logo/mark.txt");
const MARK_BLOCKS: &str = include_str!("../../proprietary/logo/mark-blocks.txt");

const BOLD: &str = "\x1b[1m";
const GREY: &str = "\x1b[90m";
const AMBER: &str = "\x1b[38;5;214m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    /// Colour and symbols; off = plain text.
    pub rich: bool,
}

impl Style {
    pub fn stdout() -> Self {
        Self::of(&std::io::stdout())
    }

    pub fn stderr() -> Self {
        Self::of(&std::io::stderr())
    }

    pub fn plain() -> Self {
        Self { rich: false }
    }

    /// The same decision anstream makes for the stream (TTY, `NO_COLOR`,
    /// `CLICOLOR_FORCE`), so symbols never show where colour wouldn't.
    fn of(stream: &impl RawStream) -> Self {
        Self {
            rich: anstream::AutoStream::choice(stream) != ColorChoice::Never,
        }
    }

    fn paint(self, code: &str, s: &str) -> String {
        if self.rich {
            format!("{code}{s}{RESET}")
        } else {
            s.to_owned()
        }
    }

    pub fn strong(self, s: &str) -> String {
        self.paint(BOLD, s)
    }

    pub fn dim(self, s: &str) -> String {
        self.paint(GREY, s)
    }

    pub fn amber(self, s: &str) -> String {
        self.paint(AMBER, s)
    }

    pub fn red(self, s: &str) -> String {
        self.paint(RED, s)
    }

    /// `rich` on a terminal, `plain` otherwise.
    pub fn sym(self, rich: &'static str, plain: &'static str) -> &'static str {
        if self.rich { rich } else { plain }
    }

    /// The ✓ line of a command that worked.
    pub fn done(self, msg: &str) -> String {
        if self.rich {
            format!("{} {msg}", self.strong("✓"))
        } else {
            msg.to_owned()
        }
    }

    /// The ✗ line of an error, with the fix indented under it.
    pub fn error(self, what: &str, hint: &str) -> String {
        let mut out = if self.rich {
            format!("{} {what}\n", self.red("✗"))
        } else {
            format!("error: {what}\n")
        };
        for line in hint.lines().filter(|l| !l.is_empty()) {
            let _ = writeln!(out, "  {}", self.dim(line));
        }
        out
    }

    /// `label` padded to the column the values start in, in grey.
    pub fn field(self, label: &str, value: &str) -> String {
        format!("{}{value}", self.dim(&format!("{label:<10}")))
    }

    /// `text` as a terminal hyperlink to `url` (OSC 8). Terminals without
    /// them show just the text, so selecting it copies the same characters.
    pub fn link(self, url: &str, text: &str) -> String {
        if self.rich {
            format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
        } else {
            text.to_owned()
        }
    }
}

/// The braille R, or half blocks on the classic Windows console, whose
/// fonts have no braille (Windows Terminal sets `WT_SESSION` and has them).
fn mark() -> &'static str {
    if cfg!(windows) && std::env::var_os("WT_SESSION").is_none() {
        MARK_BLOCKS
    } else {
        MARK
    }
}

/// The R with `lines` beside it; just the lines in plain text.
pub fn with_mark(style: Style, lines: &[String]) -> String {
    let mut out = String::new();
    if !style.rich {
        for l in lines {
            let _ = writeln!(out, "{}", l.trim_end());
        }
        return out;
    }
    let mark: Vec<&str> = mark().lines().collect();
    for i in 0..mark.len().max(lines.len()) {
        let m = mark.get(i).copied().unwrap_or("");
        let text = lines.get(i).map(String::as_str).unwrap_or("");
        let _ = writeln!(out, "{}   {text}", style.strong(m));
    }
    out
}

/// Columns padded to their widest cell, the header in grey.
pub fn table(style: Style, header: &[&str], rows: &[Vec<String>]) -> String {
    let width = |i: usize| {
        rows.iter()
            .map(|r| r[i].chars().count())
            .chain([header[i].len()])
            .max()
            .unwrap_or(0)
    };
    let widths: Vec<usize> = (0..header.len()).map(width).collect();
    let line = |cells: Vec<String>, lens: Vec<usize>| {
        let mut s = String::new();
        for (i, (cell, len)) in cells.iter().zip(lens).enumerate() {
            s.push_str(cell);
            if i + 1 < widths.len() {
                s.push_str(&" ".repeat(widths[i] - len + 3));
            }
        }
        s
    };
    let mut out = line(
        header.iter().map(|h| style.dim(h)).collect(),
        header.iter().map(|h| h.len()).collect(),
    );
    out.push('\n');
    for r in rows {
        let lens = r.iter().map(|c| c.chars().count()).collect();
        let mut cells = r.clone();
        cells[0] = style.strong(&cells[0]);
        out.push_str(&line(cells, lens));
        out.push('\n');
    }
    out
}

/// A `cells`-wide bar for `fraction` (0..=1).
pub fn bar(fraction: f64, cells: usize) -> String {
    let full = ((fraction.clamp(0.0, 1.0) * cells as f64).round() as usize).min(cells);
    // At least one cell once anything is used, so 0.4 % doesn't look like none.
    let full = if fraction > 0.0 { full.max(1) } else { full };
    format!("{}{}", "█".repeat(full), "░".repeat(cells - full))
}

/// "2 h 14 min", "5 min", "40 s", "3 d 2 h".
pub fn duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min", s / 60),
        s if s < 86_400 => match (s / 3600, s % 3600 / 60) {
            (h, 0) => format!("{h} h"),
            (h, m) => format!("{h} h {m} min"),
        },
        s => match (s / 86_400, s % 86_400 / 3600) {
            (d, 0) => format!("{d} d"),
            (d, h) => format!("{d} d {h} h"),
        },
    }
}

/// Decimal units, as disks and hosting plans count them.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    match u {
        0 => format!("{n} B"),
        // "500 GB", not "500.0 GB".
        _ if v.fract() < 0.05 => format!("{v:.0} {}", UNITS[u]),
        _ => format!("{v:.1} {}", UNITS[u]),
    }
}

/// "Nov 1" for the month after `month` ("2026-10").
pub fn next_month_start(month: &str) -> String {
    const NAMES: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let m: usize = month
        .get(5..7)
        .and_then(|m| m.parse().ok())
        .filter(|m| (1..=12).contains(m))
        .unwrap_or(12);
    format!("{} 1", NAMES[m % 12])
}

/// `p` with the home directory as `~`, which keeps paths short enough to
/// read at a glance.
pub fn path(p: &Path) -> String {
    let shown = match home_dir().as_deref().and_then(|h| p.strip_prefix(h).ok()) {
        Some(rest) if !rest.as_os_str().is_empty() => {
            Path::new("~").join(rest).display().to_string()
        }
        _ => p.display().to_string(),
    };
    // Windows reads / and \ alike, and a dir passed from a Unix-style shell
    // arrives with /: show one kind.
    if cfg!(windows) {
        shown.replace('/', "\\")
    } else {
        shown
    }
}

fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// `text` cut to `max` characters by dropping its middle, which keeps both
/// the start and the file name of a long path.
pub fn fit(s: Style, text: &str, max: usize) -> String {
    let n = text.chars().count();
    let dots = s.sym("…", "...");
    let room = max.saturating_sub(dots.chars().count());
    if n <= max || room < 4 {
        return text.to_owned();
    }
    let head = room / 2;
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(n - (room - head)).collect();
    format!("{start}{dots}{end}")
}

/// "Nov 24": the UTC day of a Unix time, as budgets count months in UTC.
pub fn utc_day(unix: u64) -> String {
    jiff::Timestamp::from_second(unix as i64)
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::UTC)
                .strftime("%b %-d")
                .to_string()
        })
        .unwrap_or_default()
}

/// Start and end (Unix seconds, UTC) of `month` ("2026-10").
pub fn month_bounds(month: &str) -> Option<(u64, u64)> {
    use jiff::ToSpan;
    let first: jiff::civil::Date = format!("{month}-01").parse().ok()?;
    let at = |d: jiff::civil::Date| {
        d.to_zoned(jiff::tz::TimeZone::UTC)
            .ok()
            .and_then(|z| u64::try_from(z.timestamp().as_second()).ok())
    };
    Some((at(first)?, at(first.checked_add(1.month()).ok()?)?))
}

/// A notice with its time window in this machine's time zone:
/// "Serverumzug, Nov 11 09:00 to 14:00".
pub fn notice_line(n: &reamesh_relay_proto::Notice) -> String {
    let tz = jiff::tz::TimeZone::system();
    let at = |unix: u64| {
        jiff::Timestamp::from_second(unix as i64)
            .ok()
            .map(|t| t.to_zoned(tz.clone()))
    };
    let (from, until) = (
        at(n.from).filter(|_| n.from != 0),
        at(n.until).filter(|_| n.until != 0),
    );
    let day_time = "%b %-d %H:%M";
    match (from, until) {
        (Some(a), Some(b)) if a.date() == b.date() => format!(
            "{}, {} to {}",
            n.text,
            a.strftime(day_time),
            b.strftime("%H:%M")
        ),
        (Some(a), Some(b)) => format!(
            "{}, {} to {}",
            n.text,
            a.strftime(day_time),
            b.strftime(day_time)
        ),
        (Some(a), None) => format!("{}, from {}", n.text, a.strftime(day_time)),
        (None, Some(b)) => format!("{}, until {}", n.text, b.strftime(day_time)),
        (None, None) => n.text.clone(),
    }
}

/// When something local happened: "23:39" today, "Oct 30" before.
pub fn local_since(unix: u64) -> String {
    let tz = jiff::tz::TimeZone::system();
    let Ok(t) = jiff::Timestamp::from_second(unix as i64) else {
        return String::new();
    };
    let then = t.to_zoned(tz.clone());
    if then.date() == jiff::Timestamp::now().to_zoned(tz).date() {
        then.strftime("%H:%M").to_string()
    } else {
        then.strftime("%b %-d").to_string()
    }
}

/// Puts `text` on the clipboard of the terminal this runs in (OSC 52),
/// which also reaches the desktop over SSH. False where it can't even be
/// tried (not a terminal, escapes off); whether the terminal allowed it,
/// nothing tells.
pub fn copy_to_clipboard(text: &str) -> bool {
    use std::io::{IsTerminal, Write};
    let mut out = std::io::stdout();
    // The classic Windows console would print the escape; Windows Terminal
    // understands it.
    let windows_ok = !cfg!(windows) || std::env::var_os("WT_SESSION").is_some();
    if !Style::stdout().rich || !out.is_terminal() || !windows_ok {
        return false;
    }
    // tmux takes it itself and passes it on with `set-clipboard on`.
    let seq = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
    out.write_all(seq.as_bytes())
        .and_then(|()| out.flush())
        .is_ok()
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                ABC[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

/// A relay code alone on its line, nothing around it, so a triple click
/// copies exactly the code. It is a link too: clicked on the desktop it
/// opens ReaMesh, which offers to add the relay.
pub fn code_block(s: Style, intro: &str, code: &str) -> String {
    format!("{intro}\n\n{}\n\n", s.strong(&s.link(code, code)))
}

/// Where a code goes, and who should have it.
pub fn share_hint(s: Style) -> String {
    let text = format!(
        "Paste it in ReaMesh under Settings {} Relay. It lets whoever has it host\nthrough this relay, so keep it within your circle.",
        s.sym("›", ">")
    );
    text.lines().map(|l| s.dim(l) + "\n").collect()
}

/// Why there is no code to show yet, and how to get one.
pub fn no_address_hint(s: Style, config: &Path) -> String {
    let text = format!(
        "No public address found, so a relay code would lead nowhere. Set public_addrs\nor dns_name in {} (the relay picks it up by itself),\nthen print the code with: reamesh-relay code",
        path(config)
    );
    text.lines().map(|l| s.dim(l) + "\n").collect()
}

/// An error the CLI knows how to explain: what happened, and what to do.
/// Printed as `✗ what` with the hint under it instead of an error chain.
#[derive(Debug)]
pub struct Problem {
    pub what: String,
    pub hint: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.what)
    }
}

impl std::error::Error for Problem {}

pub fn problem(what: impl Into<String>, hint: impl Into<String>) -> anyhow::Error {
    Problem {
        what: what.into(),
        hint: hint.into(),
    }
    .into()
}

pub mod log {
    //! The log line format: `HH:MM:SS  message`, the time in grey, and a
    //! line with the date whenever the day changes under a running relay.
    //! The journal stamps every line itself, so there the time is left out.
    //! Only `limit`, `budget` and `firewall` events (a `kind` field) carry a
    //! tag, in amber, and errors a red ✗.

    use std::fmt;
    use std::sync::Mutex;

    use tracing::field::{Field, Visit};
    use tracing::{Event, Level, Subscriber};
    use tracing_subscriber::fmt::format::Writer;
    use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
    use tracing_subscriber::registry::LookupSpan;

    use super::Style;

    pub struct Plain {
        style: Style,
        clock: bool,
        /// The day of the last line, for the date line.
        day: Mutex<Option<jiff::civil::Date>>,
    }

    impl Plain {
        pub fn new(style: Style) -> Self {
            Self {
                style,
                clock: !to_journal(),
                day: Mutex::new(None),
            }
        }
    }

    /// Whether stderr is connected to the systemd journal: `JOURNAL_STREAM`
    /// names its device and inode, and children inherit the variable, so it
    /// is compared with stderr itself.
    fn to_journal() -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let Some(v) = std::env::var_os("JOURNAL_STREAM") else {
                return false;
            };
            let v = v.to_string_lossy();
            let Some((dev, ino)) = v.split_once(':') else {
                return false;
            };
            match std::fs::metadata("/dev/stderr") {
                Ok(m) => dev.parse() == Ok(m.dev()) && ino.parse() == Ok(m.ino()),
                Err(_) => true,
            }
        }
        #[cfg(not(unix))]
        false
    }

    #[derive(Default)]
    struct Fields {
        message: String,
        kind: Option<String>,
    }

    impl Visit for Fields {
        fn record_str(&mut self, field: &Field, value: &str) {
            match field.name() {
                "message" => self.message = value.to_owned(),
                "kind" => self.kind = Some(value.to_owned()),
                _ => {}
            }
        }

        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            match field.name() {
                "message" => self.message = format!("{value:?}"),
                "kind" => self.kind = Some(format!("{value:?}")),
                _ => {}
            }
        }
    }

    impl<S, N> FormatEvent<S, N> for Plain
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
        N: for<'a> FormatFields<'a> + 'static,
    {
        fn format_event(
            &self,
            _: &FmtContext<'_, S, N>,
            mut w: Writer<'_>,
            event: &Event<'_>,
        ) -> fmt::Result {
            let s = self.style;
            let mut f = Fields::default();
            event.record(&mut f);
            if self.clock {
                let now = jiff::Zoned::now();
                let mut day = self.day.lock().unwrap_or_else(|e| e.into_inner());
                if day.is_some_and(|d| d != now.date()) {
                    let rule = s.sym("──", "--");
                    let date = now.strftime("%A, %B %-d");
                    writeln!(w, "{}", s.dim(&format!("{rule} {date} {rule}")))?;
                }
                *day = Some(now.date());
                write!(w, "{}  ", s.dim(&now.strftime("%H:%M:%S").to_string()))?;
            }
            if *event.metadata().level() == Level::ERROR {
                write!(
                    w,
                    "{} ",
                    if s.rich {
                        s.red("✗")
                    } else {
                        "error ".to_owned()
                    }
                )?;
            } else if let Some(kind) = &f.kind {
                write!(w, "{}  ", s.amber(kind))?;
            }
            writeln!(w, "{}", f.message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_has_no_escapes_or_symbols() {
        let s = Style::plain();
        assert_eq!(s.strong("x"), "x");
        assert_eq!(s.done("Key band added."), "Key band added.");
        assert_eq!(
            s.error("Port 4001 is already in use.", "Set another port."),
            "error: Port 4001 is already in use.\n  Set another port.\n"
        );
        assert_eq!(with_mark(s, &["ReaMesh Relay".into()]), "ReaMesh Relay\n");
        let rich = Style { rich: true };
        assert!(with_mark(rich, &["ReaMesh Relay".into()]).lines().count() >= 7);
    }

    #[test]
    fn units_and_durations_read_naturally() {
        assert_eq!(duration(Duration::from_secs(40)), "40 s");
        assert_eq!(duration(Duration::from_secs(8040)), "2 h 14 min");
        assert_eq!(duration(Duration::from_secs(7200)), "2 h");
        assert_eq!(duration(Duration::from_secs(3 * 86_400 + 7200)), "3 d 2 h");
        assert_eq!(bytes(3_200_000_000), "3.2 GB");
        assert_eq!(bytes(500_000_000_000), "500 GB");
        assert_eq!(bar(0.0, 4), "░░░░");
        assert_eq!(bar(0.001, 4), "█░░░");
        assert_eq!(bar(1.0, 4), "████");
        assert_eq!(next_month_start("2026-10"), "Nov 1");
        assert_eq!(next_month_start("2026-12"), "Jan 1");
    }

    #[test]
    fn long_text_loses_its_middle() {
        let s = Style::plain();
        assert_eq!(fit(s, "/srv/relay", 20), "/srv/relay");
        assert_eq!(
            fit(s, "/home/lena/very/long/path/config.toml", 20),
            "/home/le...nfig.toml"
        );
        assert_eq!(
            fit(s, "/home/lena/very/long/path/config.toml", 20).len(),
            20
        );
        let rich = Style { rich: true };
        assert_eq!(fit(rich, "abcdefghijklmnop", 9), "abcd…mnop");
    }

    #[test]
    fn clipboard_payload_is_base64() {
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"reamesh://relay/x"), "cmVhbWVzaDovL3JlbGF5L3g=");
    }

    #[test]
    fn months_and_days_in_utc() {
        assert_eq!(
            month_bounds("2026-11"),
            Some((1_793_491_200, 1_796_083_200))
        );
        assert_eq!(utc_day(1_796_083_199), "Nov 30");
        assert_eq!(month_bounds("bogus"), None);
    }

    #[test]
    fn plain_link_is_just_the_text() {
        assert_eq!(Style::plain().link("reamesh://relay/x", "code"), "code");
        assert!(Style { rich: true }.link("u", "t").contains("\x1b]8;;u"));
    }

    #[test]
    fn table_pads_columns() {
        let t = table(
            Style::plain(),
            &["NAME", "ADDED"],
            &[
                vec!["band".into(), "-".into()],
                vec!["a".into(), "x".into()],
            ],
        );
        assert_eq!(t, "NAME   ADDED\nband   -\na      x\n");
    }
}
