//! The `trader` binary: its command line, the bot it runs, and the bot's console.

pub mod bot;
pub mod config;
pub mod console;
mod files;
pub mod keys;
pub mod node;
pub mod stream;

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use tokio::sync::oneshot;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::bot::Link;
use crate::config::Config;
use crate::console::{Console, Prepared};
use crate::keys::AccountKey;
use crate::node::Node;

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
    /// Trades the configured account until stopped.
    Run {
        /// The config file.
        #[arg(short, long, env = "CONFIG_PATH")]
        config: PathBuf,
    },
}

/// Parses the process's arguments and runs the command; `--version` and `--help` print and exit.
/// Error cases:
/// - the config does not load (see `Config::load`)
/// - the console cannot be prepared (see `console::prepare`)
/// - see `serve`
pub fn run() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Run { config } => {
            let cfg = Config::load(&config)?;
            let filter = EnvFilter::try_new(&cfg.logging.level).context("logging.level")?;
            tracing_subscriber::fmt().with_env_filter(filter).with_ansi(std::io::stdout().is_terminal()).init();
            let console = cfg.console.as_ref().map(|section| console::prepare(section, &mut std::io::stdout()));
            let console = console.transpose()?;
            let runtime = tokio::runtime::Runtime::new().context("the async runtime")?;
            runtime.block_on(async {
                let stop = bot::stop_signal()?;
                serve(cfg, |name| std::env::var(name).ok(), stop, console).await
            })
        }
    }
}

/// Runs the bot until `stop` completes or it halts, with its console when `console` holds one
/// prepared from the config's section. The console's failure stops the bot as `stop` would.
/// Error cases:
/// - the key does not load, or the console's saved controls cannot be read or its address bound
/// - the bot fails to start, or halts (see `bot::run_linked`)
/// - the console fails
pub async fn serve(
    cfg: Config,
    env: impl Fn(&str) -> Option<String>,
    stop: impl Future<Output = ()>,
    console: Option<Prepared>,
) -> anyhow::Result<()> {
    let (Some(prepared), Some(section)) = (console, cfg.console.as_ref()) else {
        return bot::run(cfg, env, stop).await;
    };
    let key = AccountKey::load(&cfg.account.key, env).await?;
    info!(account = %key.address(), "account key unlocked");
    let (link, account) = Link::open(&cfg, key.address(), section.data_dir.clone()).await?;
    let mut console = Console::start(section, prepared, Node::from_section(&cfg.node)?, vec![account]).await?;
    let (console_failed, failed) = oneshot::channel::<()>();
    let stop = async move {
        tokio::select! {
            () = stop => {}
            _ = failed => {}
        }
    };
    let bot = bot::run_linked(cfg, key, link, stop);
    tokio::pin!(bot);
    tokio::select! {
        result = &mut bot => result,
        failure = console.failed() => {
            error!(alert = "console_failed", "the console stopped, so the bot stops too: {failure:#}");
            let _ = console_failed.send(());
            if let Err(e) = bot.await {
                error!("{e:#}");
            }
            Err(failure.context("the console stopped"))
        }
    }
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

    /// An unknown argument is refused, never ignored.
    #[test]
    fn unknown_argument_is_refused() {
        let err = Cli::try_parse_from(["trader", "--bogus"]).expect_err("an unknown flag is refused");
        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    }

    /// `run` takes its config file by `-c` or `--config`.
    #[test]
    fn run_takes_a_config_file() {
        for flag in ["-c", "--config"] {
            let cli = Cli::try_parse_from(["trader", "run", flag, "bot.toml"]).unwrap();
            let Command::Run { config } = cli.command;
            assert_eq!(config, std::path::Path::new("bot.toml"));
        }
    }
}
