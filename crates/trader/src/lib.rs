//! The `trader` binary: its command line, and the bot it runs.

pub mod accounts;
pub mod bot;
pub mod chain;
pub mod config;
pub mod keys;
pub mod node;
pub mod stream;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{Parser, Subcommand};
use engine_core::types::AccountId;
use tracing_subscriber::EnvFilter;

use crate::config::Config;

/// Log targets held at `info` whatever the configured level: below it, the RPC client logs its
/// endpoint's URL, which may carry an API key.
const QUIET: &str = "alloy_transport_http=info,reqwest=info";

/// The monkeytrading bot.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// What the bot does.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Trades the configured accounts until stopped.
    Run {
        /// The config file.
        #[arg(short, long, env = "CONFIG_PATH")]
        config: PathBuf,
    },
    /// Prints the address of every account the config names, to fund and declare them.
    Addresses {
        /// The config file.
        #[arg(short, long, env = "CONFIG_PATH")]
        config: PathBuf,
    },
    /// Declares, funds, claims MONKEY for, and withdraws from the configured accounts.
    Accounts {
        /// The config file.
        #[arg(short, long, env = "CONFIG_PATH")]
        config: PathBuf,
        /// Acts for this account only; repeat for several. Without it, for every account.
        #[arg(long = "account", value_name = "ADDRESS", global = true)]
        accounts: Vec<AccountId>,
        #[command(subcommand)]
        action: accounts::cli::Action,
    },
}

/// Parses the process's arguments and runs the command; `--version` and `--help` print and exit.
/// Error cases:
/// - the config does not load (see `Config::load`)
/// - the bot fails to start, or halts (see `bot::run`)
/// - an account action fails (see `accounts::cli::run`)
pub fn run() -> anyhow::Result<()> {
    let env = |name: &str| std::env::var(name).ok();
    match Cli::parse().command {
        Command::Run { config } => {
            let cfg = start(&config)?;
            let runtime = tokio::runtime::Runtime::new().context("the async runtime")?;
            runtime.block_on(async {
                let stop = bot::stop_signal()?;
                bot::run(cfg, env, stop).await
            })
        }
        Command::Addresses { config } => {
            let cfg = Config::load(&config)?;
            let runtime = tokio::runtime::Runtime::new().context("the async runtime")?;
            runtime.block_on(async {
                for address in bot::addresses(&cfg, |name| std::env::var(name).ok()).await? {
                    println!("{address}");
                }
                Ok(())
            })
        }
        Command::Accounts { config, accounts, action } => {
            let cfg = start(&config)?;
            let runtime = tokio::runtime::Runtime::new().context("the async runtime")?;
            let terminal = std::io::stdin().is_terminal();
            runtime.block_on(accounts::cli::run(cfg, accounts, action, env, terminal, &mut std::io::stdout()))
        }
    }
}

/// Loads the config at `path` and starts logging at its level, but for the RPC client's own logs,
/// which stay at `info` (`QUIET`).
/// Error cases:
/// - the config does not load (see `Config::load`), or its level is not a filter
fn start(path: &Path) -> anyhow::Result<Config> {
    let cfg = Config::load(path)?;
    let filter = EnvFilter::try_new(format!("{},{QUIET}", cfg.logging.level)).context("logging.level")?;
    tracing_subscriber::fmt().with_env_filter(filter).with_ansi(std::io::stdout().is_terminal()).init();
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;
    use clap::{CommandFactory, Parser};

    use crate::{Cli, Command};

    /// The command definition is internally consistent.
    #[test]
    fn command_is_well_formed() {
        Cli::command().debug_assert();
    }

    /// `--version` reports the crate's version and exits with a display-version error.
    #[test]
    fn version_flag_reports_the_package_version() {
        let err = Cli::try_parse_from(["trader", "--version"]).expect_err("--version exits through clap");
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
        assert_eq!(err.to_string(), format!("trader {}\n", env!("CARGO_PKG_VERSION")));
    }

    /// `addresses` takes its config the same way.
    #[test]
    fn addresses_takes_a_config_file() {
        let cli = Cli::try_parse_from(["trader", "addresses", "-c", "bot.toml"]).unwrap();
        let Command::Addresses { config } = cli.command else { panic!("addresses is addresses") };
        assert_eq!(config, std::path::Path::new("bot.toml"));
    }

    /// An unknown argument is refused, never ignored.
    #[test]
    fn unknown_argument_is_refused() {
        let err = Cli::try_parse_from(["trader", "--bogus"]).expect_err("an unknown flag is refused");
        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    }

    /// At any configured level, the RPC client's debug lines, which carry its endpoint's URL, are
    /// not written, while the bot's are.
    #[test]
    fn the_rpc_clients_debug_lines_are_never_written() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Default)]
        struct Lines(Arc<Mutex<Vec<u8>>>);
        impl Write for Lines {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let lines = Lines::default();
        let writer = lines.clone();
        let filter = tracing_subscriber::EnvFilter::try_new(format!("trace,{}", crate::QUIET)).unwrap();
        let subscriber = tracing_subscriber::fmt().with_env_filter(filter).with_writer(move || writer.clone()).finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!(target: "alloy_transport_http::reqwest_transport", "https://rpc.example/secret-key");
            tracing::debug!(target: "reqwest::connect", "https://rpc.example/secret-key");
            tracing::debug!(target: "trader::chain", "the bot's own line");
        });
        let written = String::from_utf8(lines.0.lock().unwrap().clone()).unwrap();
        assert!(written.contains("the bot's own line") && !written.contains("secret-key"), "{written}");
    }

    /// `run` takes its config file by `-c` or `--config`.
    #[test]
    fn run_takes_a_config_file() {
        for flag in ["-c", "--config"] {
            let cli = Cli::try_parse_from(["trader", "run", flag, "bot.toml"]).unwrap();
            let Command::Run { config } = cli.command else { panic!("run is a run") };
            assert_eq!(config, std::path::Path::new("bot.toml"));
        }
    }
}
