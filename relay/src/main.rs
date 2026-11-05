use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{Parser, Subcommand, ValueEnum};
use reamesh_relay::addrs;
use reamesh_relay::config::{self, Config};
use reamesh_relay::seen::Seen;
use reamesh_relay::server::{self, Options, Stopped};
use reamesh_relay::status::Status;
use reamesh_relay::term::{self, Problem, Style};
use reamesh_relay::traffic::Ledger;
use reamesh_relay::update::{self, Checked};
use reamesh_relay_proto::{RelayAddr, RelayCode};

// The top-level help, laid out by hand: clap's own lists `key` as one command
// and has no examples section. The escapes are stripped by clap wherever
// colour is off.
const HELP: &str = concat!(
    "\x1b[1mReaMesh Relay ",
    env!("CARGO_PKG_VERSION"),
    "\x1b[0m\n",
    "\x1b[90mLets ReaMesh hosts be reached across routers and firewalls.\x1b[0m\n\n",
    "\x1b[1mUsage\x1b[0m\n  {usage}\n\n",
    "\x1b[1mCommands\x1b[0m\n",
    "  \x1b[1mrun\x1b[0m                    Run the relay (what it does without a command)\n",
    "  \x1b[1mstatus\x1b[0m [--watch]       Whether it runs, this month's traffic, the hosts online\n",
    "  \x1b[1mcode\x1b[0m [name] [--copy]   Print the relay code for a key (every key without a name)\n",
    "  \x1b[1mkey add\x1b[0m <name>         Create a key and print its relay code\n",
    "  \x1b[1mkey list\x1b[0m               List the keys and when each was last used\n",
    "  \x1b[1mkey expires\x1b[0m <name> <when>  Change a key's last day (a date, 30d, never)\n",
    "  \x1b[1mkey remove\x1b[0m <name>      Delete a key; hosts using it lose access within a few seconds\n",
    "  \x1b[1mlist\x1b[0m [on|off]          Whether everyone with a key sees the sessions hosted here\n",
    "  \x1b[1mnotice\x1b[0m [set|clear]     Show, set or clear the announcement hosts see in ReaMesh\n",
    "  \x1b[1malert test\x1b[0m             Try the alert command from config.toml\n",
    "  \x1b[1mupdate\x1b[0m                 Get the newest version now\n\n",
    "\x1b[1mOptions\x1b[0m\n{options}\n\n",
    "\x1b[1mExamples\x1b[0m\n",
    "  reamesh-relay                    \x1b[90mRun it; the first start prints the relay code\x1b[0m\n",
    "  reamesh-relay key add band       \x1b[90mA key for your band, and its relay code\x1b[0m\n",
    "  reamesh-relay code band --copy   \x1b[90mPut it on your clipboard, also over SSH\x1b[0m\n",
    "  reamesh-relay notice set \"Moving servers\" --from \"2026-11-11 09:00\" --until \"2026-11-11 14:00\"\n",
    "  reamesh-relay status -w          \x1b[90mKeep the status on screen, updated live\x1b[0m\n",
    "  reamesh-relay --data /srv/rr     \x1b[90mKeep the identity, keys and config in /srv/rr\x1b[0m\n",
);

fn styles() -> Styles {
    Styles::styled()
        .header(Effects::BOLD.into())
        .usage(Effects::BOLD.into())
        .literal(Effects::BOLD.into())
        .placeholder(AnsiColor::BrightBlack.on_default())
        .valid(Effects::BOLD.into())
        .invalid(AnsiColor::Red.on_default())
        .error(AnsiColor::Red.on_default())
}

#[derive(Parser)]
#[command(
    name = "reamesh-relay",
    version,
    about = "Lets ReaMesh hosts be reached across routers and firewalls.",
    help_template = HELP,
    styles = styles(),
    disable_help_subcommand = true
)]
struct Cli {
    /// Where the identity, keys and config live.
    #[arg(long, global = true, env = "REAMESH_RELAY_DATA", value_name = "DIR")]
    data: Option<PathBuf>,
    /// plain (the default), or json for log collectors.
    #[arg(
        long,
        global = true,
        value_enum,
        default_value_t = LogFormat::Plain,
        value_name = "FORMAT",
        hide_possible_values = true,
        hide_default_value = true
    )]
    log_format: LogFormat,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum LogFormat {
    Plain,
    Json,
}

#[derive(Subcommand)]
enum Command {
    /// Run the relay (what it does without a command).
    Run,
    /// Whether it runs, this month's traffic, the hosts online.
    Status {
        /// Keep it on screen and update it every 2 seconds.
        #[arg(short, long)]
        watch: bool,
    },
    /// Print the relay code for a key (every key without a name).
    Code {
        name: Option<String>,
        /// Also put it on the clipboard (OSC 52; works over SSH in most terminals).
        #[arg(long)]
        copy: bool,
    },
    /// Create, list and delete access keys.
    Key {
        #[command(subcommand)]
        command: KeyCommand,
    },
    /// Whether everyone with a key sees the sessions hosted here.
    List {
        #[arg(value_enum)]
        state: Option<ListState>,
    },
    /// Show, set or clear the announcement hosts see in ReaMesh.
    Notice {
        #[command(subcommand)]
        command: Option<NoticeCommand>,
    },
    /// Try the alert command from config.toml.
    Alert {
        #[command(subcommand)]
        command: AlertCommand,
    },
    /// Get the newest version now; a running relay restarts into it once nothing
    /// is being relayed.
    Update,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ListState {
    On,
    Off,
}

#[derive(Subcommand)]
enum NoticeCommand {
    /// Announce something, like downtime. Replaces the current notice.
    Set {
        text: String,
        /// When it starts, like "2026-11-11 09:00" (this machine's time zone).
        #[arg(long, value_name = "TIME")]
        from: Option<String>,
        /// When it ends; after that ReaMesh stops showing it.
        #[arg(long, value_name = "TIME")]
        until: Option<String>,
    },
    /// Remove the notice.
    Clear,
}

#[derive(Subcommand)]
enum AlertCommand {
    /// Run the command once, and say whether it worked.
    Test,
}

#[derive(Subcommand)]
enum KeyCommand {
    /// Create a key and print its relay code.
    Add {
        name: String,
        /// Also put the code on the clipboard.
        #[arg(long)]
        copy: bool,
        /// The last day it works: a date like 2026-12-24, or a span like 30d or 6w.
        #[arg(long, value_name = "WHEN")]
        expires: Option<String>,
    },
    /// List the keys.
    List,
    /// Change a key's last day: a date, a span like 30d, or "never".
    Expires { name: String, when: String },
    /// Delete a key; hosts using it lose access within a few seconds.
    Remove { name: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let dir = cli.data.unwrap_or_else(config::default_data_dir);
    let result = match cli.command.unwrap_or(Command::Run) {
        Command::Run => run(dir, cli.log_format).map(|()| ExitCode::SUCCESS),
        Command::Status { watch: false } => status(&dir),
        Command::Status { watch: true } => watch(&dir),
        Command::Code { name, copy } => {
            code(&dir, name.as_deref(), copy).map(|()| ExitCode::SUCCESS)
        }
        Command::Key { command } => key(&dir, command).map(|()| ExitCode::SUCCESS),
        Command::List { state } => list(&dir, state).map(|()| ExitCode::SUCCESS),
        Command::Notice { command } => notice(&dir, command).map(|()| ExitCode::SUCCESS),
        Command::Alert {
            command: AlertCommand::Test,
        } => alert_test(&dir),
        Command::Update => update_now(&dir).map(|()| ExitCode::SUCCESS),
    };
    result.unwrap_or_else(|e| {
        let (what, hint) = match e.chain().find_map(|c| c.downcast_ref::<Problem>()) {
            Some(p) => (p.what.clone(), p.hint.clone()),
            None => (format!("{e:#}"), String::new()),
        };
        anstream::eprint!("{}", Style::stderr().error(&what, &hint));
        ExitCode::FAILURE
    })
}

fn notice(dir: &Path, command: Option<NoticeCommand>) -> anyhow::Result<()> {
    config::init(dir)?;
    let s = Style::stdout();
    match command {
        None => {
            let cfg = Config::load(dir)?;
            match cfg.notice {
                Some(n) => {
                    let (from, until) = n.window();
                    let line = term::notice_line(&reamesh_relay_proto::Notice {
                        text: n.text,
                        from,
                        until,
                    });
                    let over = until != 0 && until <= unix_now();
                    anstream::println!(
                        "{line}{}",
                        if over {
                            s.dim(" (over)")
                        } else {
                            String::new()
                        }
                    );
                }
                None => anstream::println!(
                    "No notice. {}",
                    s.dim(
                        "Set one with: reamesh-relay notice set <text> --from <time> --until <time>"
                    )
                ),
            }
        }
        Some(NoticeCommand::Set { text, from, until }) => {
            config::validate_notice_text(&text)
                .map_err(|e| term::problem(format!("{e:#}."), ""))?;
            let from = from.as_deref().map(config::parse_notice_time).transpose()?;
            let until = until
                .as_deref()
                .map(config::parse_notice_time)
                .transpose()?;
            if let (Some(a), Some(b)) = (from, until)
                && b <= a
            {
                return Err(term::problem("--until is not after --from.", ""));
            }
            config::set_notice(dir, Some((&text, from, until)))?;
            anstream::println!(
                "{}",
                s.done("Notice set. Hosts see it within a few seconds; a running relay picks it up by itself.")
            );
        }
        Some(NoticeCommand::Clear) => {
            config::set_notice(dir, None)?;
            anstream::println!("{}", s.done("Notice removed."));
        }
    }
    Ok(())
}

fn update_now(dir: &Path) -> anyhow::Result<()> {
    config::init(dir)?;
    let s = Style::stdout();
    let cfg = Config::load(dir)?;
    match update::check(dir, &cfg.update.channel, true)? {
        Checked::UpToDate | Checked::Available(_) => {
            anstream::println!(
                "{}",
                s.done(&format!("Version {} is the newest here.", update::VERSION))
            );
        }
        Checked::Staged(v) => {
            anstream::println!(
                "{}",
                s.done(&format!(
                    "Version {v} is ready. A running relay restarts into it once nothing is being relayed; otherwise the next start uses it."
                ))
            );
        }
    }
    Ok(())
}

fn alert_test(dir: &Path) -> anyhow::Result<ExitCode> {
    config::init(dir)?;
    let s = Style::stdout();
    let cfg = Config::load(dir)?;
    let Some(command) = cfg.alerts.command else {
        return Err(term::problem(
            "No alert command is set.",
            "Set command under [alerts] in config.toml.",
        ));
    };
    let status =
        reamesh_relay::alert::spawn(&command, "test", "This is a test of the relay's alerts.")
            .and_then(|mut child| child.wait())
            .context("running the alert command")?;
    if status.success() {
        anstream::println!("{}", s.done("The alert command ran."));
        Ok(ExitCode::SUCCESS)
    } else {
        Err(term::problem(
            format!("The alert command failed: {status}."),
            "",
        ))
    }
}

fn key(dir: &Path, command: KeyCommand) -> anyhow::Result<()> {
    config::init(dir)?;
    let s = Style::stdout();
    match command {
        KeyCommand::Expires { name, when } => {
            let date = if when.trim().eq_ignore_ascii_case("never") {
                None
            } else {
                Some(config::parse_expiry(&when, config::local_today())?)
            };
            config::set_key_expiry(dir, &name, date)?;
            let text = match date {
                Some(d) => format!("Key {} works through {d}.", s.strong(&name)),
                None => format!("Key {} no longer expires.", s.strong(&name)),
            };
            anstream::println!("{}", s.done(&text));
        }
        KeyCommand::Add {
            name,
            copy,
            expires,
        } => {
            let expires = expires
                .as_deref()
                .map(|w| config::parse_expiry(w, config::local_today()))
                .transpose()?;
            config::add_key_until(dir, &name, expires)?;
            let cfg = Config::load(dir)?;
            let code = relay_code(dir, &cfg, &name)?;
            let done = s.done(&format!("Key {} added.", s.strong(&name)));
            if code.addr.is_usable() {
                anstream::print!(
                    "{}",
                    term::code_block(s, &format!("{done} Relay code:"), &code.encode())
                );
                if copy {
                    copy_code(s, &code.encode());
                }
            } else {
                anstream::println!("{done}\n");
                anstream::print!(
                    "{}",
                    term::no_address_hint(s, &dir.join(config::CONFIG_FILE))
                );
            }
        }
        KeyCommand::List => {
            let cfg = Config::load(dir)?;
            if cfg.keys.is_empty() {
                anstream::println!(
                    "No keys. {}",
                    s.dim("Add one with: reamesh-relay key add <name>")
                );
                return Ok(());
            }
            let now = unix_now();
            let online: Vec<String> = Status::read(dir)
                .filter(|st| st.is_live(now))
                .map(|st| st.hosts.into_iter().map(|h| h.label).collect())
                .unwrap_or_default();
            let seen = Seen::load(dir).last;
            let ledger = Ledger::load(&dir.join(config::TRAFFIC_FILE));
            let today = config::local_today();
            // Only what anyone uses gets a column.
            let any_expiry = cfg.keys.iter().any(|k| k.expires.is_some());
            let any_traffic = cfg.keys.iter().any(|k| ledger.key_sent(&k.label) > 0);
            let rows: Vec<Vec<String>> = cfg
                .keys
                .iter()
                .map(|k| {
                    let fp = config::parse_key(&k.key)
                        .map(|key| config::fingerprint(&key))
                        .unwrap_or_default();
                    let added = k.added.as_ref().map_or("-".to_owned(), ToString::to_string);
                    let last = if online.contains(&k.label) {
                        "online".to_owned()
                    } else {
                        match seen.get(&k.label) {
                            Some(t) => format!(
                                "{} ago",
                                term::duration(Duration::from_secs(now.saturating_sub(*t)))
                            ),
                            None => "never".to_owned(),
                        }
                    };
                    let mut row = vec![k.label.clone(), fp, added];
                    if any_expiry {
                        row.push(match k.expiry_date() {
                            Some(d) if k.is_expired(today) => format!("{d} (expired)"),
                            Some(d) => d.to_string(),
                            None => "-".to_owned(),
                        });
                    }
                    row.push(last);
                    if any_traffic {
                        row.push(term::bytes(ledger.key_sent(&k.label)));
                    }
                    row
                })
                .collect();
            let mut header = vec!["NAME", "FINGERPRINT", "ADDED"];
            if any_expiry {
                header.push("LAST DAY");
            }
            header.push("LAST SEEN");
            if any_traffic {
                header.push("THIS MONTH");
            }
            anstream::print!("{}", term::table(s, &header, &rows));
        }
        KeyCommand::Remove { name } => {
            config::remove_key(dir, &name)?;
            anstream::println!(
                "{}",
                s.done(&format!(
                    "Key {} removed. Hosts using it lose access within a few seconds.",
                    s.strong(&name)
                ))
            );
        }
    }
    Ok(())
}

/// Sets `session_list`; a running relay picks it up within a few seconds.
fn list(dir: &Path, state: Option<ListState>) -> anyhow::Result<()> {
    config::init(dir)?;
    let s = Style::stdout();
    let on = match state {
        Some(state) => {
            config::set_session_list(dir, state == ListState::On)?;
            state == ListState::On
        }
        None => Config::load(dir)?.session_list,
    };
    let (head, tail) = if on {
        (
            "Session list on.",
            "Everyone with a key sees the sessions hosted here. Hosts choose per session.",
        )
    } else {
        (
            "Session list off.",
            "This relay keeps no list and sees no session names.",
        )
    };
    let head = if state.is_some() {
        s.done(head)
    } else {
        s.strong(head)
    };
    anstream::println!("{head} {tail}");
    Ok(())
}

fn run(dir: PathBuf, format: LogFormat) -> anyhow::Result<()> {
    init_logging(format);
    // A newer downloaded copy takes over from here. Only `run` does this: the
    // other commands may run as root, and the copy is the service user's file.
    update::start_staged(&dir);
    let rt = tokio::runtime::Runtime::new()?;
    let stopped = rt.block_on(async {
        let first_start = config::init(&dir)?;
        let mut opts = Options::new(&dir);
        opts.first_start = first_start;
        // JSON logs are for machines; the banner would only get in the way.
        opts.banner = format == LogFormat::Plain;
        opts.updates = true;
        let handle = server::spawn(opts)?;
        let stop = handle.shutdown_sender();
        tokio::spawn(async move {
            shutdown_signal().await;
            let _ = stop.send(true);
        });
        handle.join().await
    })?;
    // Everything is stopped and saved. The ports need a moment to close.
    drop(rt);
    if stopped == Stopped::ForUpdate {
        std::thread::sleep(Duration::from_millis(500));
        update::start_staged(&dir);
        // Still here: no copy to start after all. Let the supervisor restart us.
        return Err(term::problem("Could not start the new version.", ""));
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

fn init_logging(format: LogFormat) {
    use tracing_subscriber::EnvFilter;
    // libp2p's own logs stay quiet: at debug level they include addresses.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("reamesh_relay=info,warn,libp2p_swarm=error"));
    let fmt = tracing_subscriber::fmt().with_env_filter(filter);
    match format {
        LogFormat::Plain => fmt
            .event_format(term::log::Plain::new(Style::stderr()))
            .with_writer(anstream::stderr)
            .init(),
        LogFormat::Json => fmt
            .json()
            .flatten_event(true)
            .with_current_span(false)
            .with_span_list(false)
            .with_writer(std::io::stderr)
            .init(),
    }
}

fn status(dir: &Path) -> anyhow::Result<ExitCode> {
    let (out, live) = status_text(dir, Style::stdout());
    anstream::print!("{out}");
    Ok(if live {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// What `status` prints, and whether the relay runs.
fn status_text(dir: &Path, s: Style) -> (String, bool) {
    let now = unix_now();
    match Status::read(dir).filter(|st| st.is_live(now)) {
        Some(st) => (st.render(now, s), true),
        None => {
            let dot = if s.rich {
                format!("{} ", s.dim("○"))
            } else {
                String::new()
            };
            let out = format!(
                "{dot}{}\n{}\n",
                s.strong("not running"),
                s.dim("Start it with reamesh-relay or systemctl start reamesh-relay")
            );
            (out, false)
        }
    }
}

/// `status --watch`: the status redrawn in place every 2 seconds until
/// Ctrl+C, which leaves the last frame on screen. A relay that stops or
/// starts meanwhile just shows up as such.
fn watch(dir: &Path) -> anyhow::Result<ExitCode> {
    use std::io::IsTerminal;
    let s = Style::stdout();
    if !s.rich || !std::io::stdout().is_terminal() {
        return Err(term::problem(
            "--watch needs a terminal.",
            "Without one, run reamesh-relay status, or read status.json in the data dir.",
        ));
    }
    // The first frame starts on a cleared screen; later ones draw over it.
    anstream::print!("\x1b[2J");
    loop {
        let (out, _) = status_text(dir, s);
        // Home, then each line cleared to its end and the rest of the screen
        // below: redrawn over the old frame without a blank flash between.
        let mut frame = String::from("\x1b[H");
        for line in out.lines().chain(["", &s.dim("Ctrl+C to stop")]) {
            frame += line;
            frame += "\x1b[K\n";
        }
        frame += "\x1b[J";
        anstream::print!("{frame}");
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// The line after a code that `--copy` sent along.
fn copy_code(s: Style, code: &str) {
    if term::copy_to_clipboard(code) {
        anstream::println!(
            "{} {}",
            s.done("Sent to the clipboard."),
            s.dim("Nothing there? Then the terminal blocks it; copy the code above.")
        );
    } else {
        anstream::println!(
            "{}",
            s.dim("This output can't reach a clipboard; copy the code above.")
        );
    }
}

fn code(dir: &Path, name: Option<&str>, copy: bool) -> anyhow::Result<()> {
    config::init(dir)?;
    let cfg = Config::load(dir)?;
    let names: Vec<String> = match name {
        Some(n) => {
            if cfg.key(n).is_none() {
                return Err(config::no_such_key(n));
            }
            vec![n.to_owned()]
        }
        None => cfg.keys.iter().map(|k| k.label.clone()).collect(),
    };
    let s = Style::stdout();
    if names.is_empty() {
        anstream::println!(
            "No keys. {}",
            s.dim("Add one with: reamesh-relay key add <name>")
        );
        return Ok(());
    }
    if copy && names.len() > 1 {
        return Err(term::problem(
            "--copy takes one code.",
            "Name the key: reamesh-relay code <name> --copy",
        ));
    }
    let mut out = String::new();
    let mut last = String::new();
    for n in &names {
        let code = relay_code(dir, &cfg, n)?;
        if !code.addr.is_usable() {
            anstream::print!(
                "{}",
                term::no_address_hint(s, &dir.join(config::CONFIG_FILE))
            );
            return Ok(());
        }
        let intro = format!("{}{}{}", s.dim("Relay code for "), s.strong(n), s.dim(":"));
        last = code.encode();
        out += &term::code_block(s, &intro, &last);
    }
    out += &term::share_hint(s);
    anstream::print!("{out}");
    if copy {
        anstream::println!();
        copy_code(s, &last);
    }
    Ok(())
}

/// The code for key `name` as the relay hands it out: the configured and
/// detected public addresses, plus those its hosts saw while it ran.
fn relay_code(dir: &Path, cfg: &Config, name: &str) -> anyhow::Result<RelayCode> {
    let identity = config::load_identity(dir)?;
    let observed = Status::read(dir)
        .map(|s| s.observed_ips)
        .unwrap_or_default();
    Ok(RelayCode {
        addr: RelayAddr {
            peer: identity.public().to_peer_id(),
            ips: addrs::advertised(&cfg.public_addrs, &observed),
            dns: cfg.dns_name.clone().unwrap_or_default(),
            tcp: cfg.tcp_port,
            quic: cfg.quic_port,
        },
        key: cfg.key(name).ok_or_else(|| config::no_such_key(name))?,
        label: name.to_owned(),
        list: cfg.session_list,
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
