use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use reamesh_relay::addrs;
use reamesh_relay::config::{self, Config};
use reamesh_relay::proto::RelayCode;
use reamesh_relay::server::{self, Options};
use reamesh_relay::status::Status;

#[derive(Parser)]
#[command(
    version,
    about = "Self-hosted relay for ReaMesh sessions across NATs and firewalls"
)]
struct Cli {
    /// Data directory (identity, config.toml, traffic counter, status).
    #[arg(long, global = true, env = "REAMESH_RELAY_DATA")]
    data: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the relay (the default).
    Run,
    /// Show the running relay's hosts, traffic and mode.
    Status,
    /// Print the relay code for a key (every key without a label).
    Code { label: Option<String> },
    /// Manage access keys.
    Key {
        #[command(subcommand)]
        command: KeyCommand,
    },
}

#[derive(Subcommand)]
enum KeyCommand {
    /// Create a key and print its relay code.
    Add { label: String },
    /// List the keys.
    List,
    /// Delete a key; a running relay disconnects its hosts within a few seconds.
    Remove { label: String },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let dir = cli.data.unwrap_or_else(config::default_data_dir);
    match cli.command.unwrap_or(Command::Run) {
        Command::Run => run(dir),
        Command::Status => status(&dir),
        Command::Code { label } => code(&dir, label.as_deref()),
        Command::Key { command } => {
            config::init(&dir)?;
            match command {
                KeyCommand::Add { label } => {
                    config::add_key(&dir, &label)?;
                    println!("Added key \"{label}\".");
                    code(&dir, Some(&label))
                }
                KeyCommand::List => {
                    let cfg = Config::load(&dir)?;
                    if cfg.keys.is_empty() {
                        println!("No keys. Add one: reamesh-relay key add <label>");
                    }
                    for (label, key) in cfg.keys() {
                        // A fingerprint, so keys can be told apart without
                        // showing them.
                        let fp = &blake3::hash(&key).to_hex()[..8];
                        println!("{label:<32} {fp}");
                    }
                    Ok(())
                }
                KeyCommand::Remove { label } => {
                    config::remove_key(&dir, &label)?;
                    println!(
                        "Removed key \"{label}\". A running relay disconnects its hosts within a few seconds."
                    );
                    Ok(())
                }
            }
        }
    }
}

fn run(dir: PathBuf) -> anyhow::Result<()> {
    init_logging();
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        if config::init(&dir)? {
            println!("Created a new relay in {}", dir.display());
        }
        let handle = server::spawn(Options::new(&dir))?;
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

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    // libp2p's own logs stay quiet: at debug level they include addresses.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("reamesh_relay=info,warn,libp2p_swarm=error"));
    use std::io::IsTerminal;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        // Plain text for journald and `docker logs`.
        .with_ansi(std::io::stderr().is_terminal())
        .init();
}

fn status(dir: &Path) -> anyhow::Result<()> {
    let Some(s) = Status::read(dir) else {
        bail!(
            "no status in {} (is the relay running with this --data dir?)",
            dir.display()
        );
    };
    print!("{}", s.render(unix_now()));
    Ok(())
}

fn code(dir: &Path, label: Option<&str>) -> anyhow::Result<()> {
    config::init(dir)?;
    let cfg = Config::load(dir)?;
    let key = config::load_identity(dir)?;
    let status = Status::read(dir);
    let observed = status
        .as_ref()
        .map(|s| s.observed_ips.clone())
        .unwrap_or_default();
    let labels: Vec<String> = match label {
        Some(l) => {
            if cfg.key(l).is_none() {
                bail!("no key labeled {l:?} (see: reamesh-relay key list)");
            }
            vec![l.to_owned()]
        }
        None => cfg.keys.iter().map(|k| k.label.clone()).collect(),
    };
    let codes: Vec<RelayCode> = labels
        .iter()
        .map(|l| {
            Ok(RelayCode {
                peer_id: key.public().to_peer_id(),
                ips: addrs::advertised(&cfg.public_addrs, &observed),
                dns: cfg.dns_name.clone(),
                tcp_port: cfg.tcp_port,
                quic_port: cfg.quic_port,
                key: cfg.key(l).context("key vanished")?,
                label: l.clone(),
            })
        })
        .collect::<anyhow::Result<_>>()?;
    server::print_code_banner(&codes, dir);
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
