//! The `trader` binary: its command line, the bot it runs, and the bot's console.

pub mod accounts;
pub mod bot;
pub mod chain;
pub mod config;
pub mod console;
mod files;
pub mod keys;
pub mod logging;
pub mod node;
pub mod stream;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use clap::{Parser, Subcommand};
use engine_core::types::AccountId;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::Config;
use crate::console::Prepared;
use crate::logging::{Redacting, Secrets};

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
/// - the console cannot be prepared (see `console::prepare`)
/// - the bot fails to start, or halts (see `serve`)
/// - an account action fails (see `accounts::cli::run`)
pub fn run() -> anyhow::Result<()> {
    let env = |name: &str| std::env::var(name).ok();
    match Cli::parse().command {
        Command::Run { config } => {
            let (cfg, secrets) = start(&config)?;
            let console = cfg.console.as_ref().map(|section| console::prepare(section, &mut std::io::stdout()));
            let console = console.transpose()?;
            let runtime = tokio::runtime::Runtime::new().context("the async runtime")?;
            let result = runtime.block_on(async {
                let stop = bot::stop_signal()?;
                serve(cfg, env, stop, console).await
            });
            redacted(result, &secrets)
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
            let (cfg, secrets) = start(&config)?;
            let runtime = tokio::runtime::Runtime::new().context("the async runtime")?;
            let terminal = std::io::stdin().is_terminal();
            let mut out = Redacting::new(std::io::stdout(), secrets.clone());
            let result = runtime.block_on(accounts::cli::run(cfg, accounts, action, env, terminal, &mut out));
            redacted(result, &secrets)
        }
    }
}

/// Runs the bot until `stop` completes or it halts, with its console when `console` holds one
/// prepared from the config's section.
/// Error cases:
/// - see `bot::run_with_console`
pub async fn serve(
    cfg: Config,
    env: impl Fn(&str) -> Option<String> + Sync,
    stop: impl Future<Output = ()>,
    console: Option<Prepared>,
) -> anyhow::Result<()> {
    bot::run_with_console(cfg, env, stop, console).await
}

/// Loads the config at `path` and starts logging at its level, with no secret of the config in any
/// line (see `logging`); the config, and its secrets.
/// Error cases:
/// - the config does not load (see `Config::load`), or its level is not a filter
fn start(path: &Path) -> anyhow::Result<(Config, Secrets)> {
    let cfg = Config::load(path)?;
    let secrets = Secrets::of(&cfg);
    logging::subscriber(&cfg.logging.level, std::io::stdout().is_terminal(), std::io::stdout, secrets.clone())?.init();
    Ok((cfg, secrets))
}

/// `result`, its error printed as it is but for `secrets`.
fn redacted(result: anyhow::Result<()>, secrets: &Secrets) -> anyhow::Result<()> {
    result.map_err(|e| anyhow!(secrets.redact(&format!("{e:#}"))))
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

    /// A command's error is printed with no secret of the config in it, its causes included.
    #[test]
    fn errors_are_printed_without_secrets() {
        let key = "k3y0f32charactersfromaprovider00";
        let secrets = crate::logging::Secrets::of_urls([format!("https://rpc.example/v2/{key}").as_str()]);
        let error = anyhow::anyhow!("no answer from https://rpc.example/v2/{key}").context("reading the vault");
        let printed = format!("{:#}", crate::redacted(Err(error), &secrets).unwrap_err());
        assert_eq!(printed, "reading the vault: no answer from [redacted]");
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
