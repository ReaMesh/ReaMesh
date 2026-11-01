use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{Parser, Subcommand, ValueEnum};
use reamesh_relay::addrs;
use reamesh_relay::config::{self, Config};
use reamesh_relay::seen::Seen;
use reamesh_relay::server::{self, Options};
use reamesh_relay::status::Status;
use reamesh_relay::term::{self, Problem, Style};
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
    "  \x1b[1mkey remove\x1b[0m <name>      Delete a key; hosts using it lose access within a few seconds\n\n",
    "\x1b[1mOptions\x1b[0m\n{options}\n\n",
    "\x1b[1mExamples\x1b[0m\n",
    "  reamesh-relay                    \x1b[90mRun it; the first start prints the relay code\x1b[0m\n",
    "  reamesh-relay key add band       \x1b[90mA key for your band, and its relay code\x1b[0m\n",
    "  reamesh-relay code band --copy   \x1b[90mPut it on your clipboard, also over SSH\x1b[0m\n",
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
}

#[derive(Subcommand)]
enum KeyCommand {
    /// Create a key and print its relay code.
    Add {
        name: String,
        /// Also put the code on the clipboard.
        #[arg(long)]
        copy: bool,
    },
    /// List the keys.
    List,
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

fn key(dir: &Path, command: KeyCommand) -> anyhow::Result<()> {
    config::init(dir)?;
    let s = Style::stdout();
    match command {
        KeyCommand::Add { name, copy } => {
            config::add_key(dir, &name)?;
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
                    vec![k.label.clone(), fp, added, last]
                })
                .collect();
            anstream::print!(
                "{}",
                term::table(s, &["NAME", "FINGERPRINT", "ADDED", "LAST SEEN"], &rows)
            );
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

fn run(dir: PathBuf, format: LogFormat) -> anyhow::Result<()> {
    init_logging(format);
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let first_start = config::init(&dir)?;
        let mut opts = Options::new(&dir);
        opts.first_start = first_start;
        // JSON logs are for machines; the banner would only get in the way.
        opts.banner = format == LogFormat::Plain;
        let handle = server::spawn(opts)?;
        let stop = handle.shutdown_sender();
        tokio::spawn(async move {
            shutdown_signal().await;
            let _ = stop.send(true);
        });
        handle.join().await
    })
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
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
