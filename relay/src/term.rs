//! Terminal output, in the app's palette: greys only, bold for what matters,
//! amber for the warnings an operator has to act on (`limit`, `budget`), red
//! only for the ✗ of an error. Colour and symbols go together and both drop
//! out when the stream isn't a terminal or `NO_COLOR` is set, so `docker logs`
//! and journald get plain text.
//!
//! Everything is written through `anstream`, which also turns the escapes into
//! console calls on an old Windows console.

use std::fmt::Write as _;
use std::path::Path;
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

/// The local time of day, for log lines.
pub fn clock() -> String {
    jiff::Zoned::now().strftime("%H:%M:%S").to_string()
}

/// `p` with the home directory as `~`, which keeps paths short enough to
/// read at a glance.
pub fn path(p: &Path) -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    match home.as_deref().and_then(|h| p.strip_prefix(h).ok()) {
        Some(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => p.display().to_string(),
    }
}

/// A relay code alone on its line, nothing around it, so a triple click
/// copies exactly the code.
pub fn code_block(s: Style, intro: &str, code: &str) -> String {
    format!("{intro}\n\n{}\n\n", s.strong(code))
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
    //! The log line format: `HH:MM:SS  message`, the time in grey. Only
    //! `limit` and `budget` events (a `kind` field) carry a tag, in amber,
    //! and errors a red ✗.

    use std::fmt;

    use tracing::field::{Field, Visit};
    use tracing::{Event, Level, Subscriber};
    use tracing_subscriber::fmt::format::Writer;
    use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
    use tracing_subscriber::registry::LookupSpan;

    use super::Style;

    pub struct Plain {
        pub style: Style,
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
            write!(w, "{}  ", s.dim(&super::clock()))?;
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
