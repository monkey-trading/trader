//! The `trader` binary: its command line, and the bot it runs.

pub mod bot;
pub mod config;
pub mod keys;
pub mod node;
pub mod stream;

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::Config;

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
/// - the bot fails to start, or halts (see `bot::run`)
pub fn run() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Run { config } => {
            let cfg = Config::load(&config)?;
            let filter = EnvFilter::try_new(&cfg.logging.level).context("logging.level")?;
            tracing_subscriber::fmt().with_env_filter(filter).with_ansi(std::io::stdout().is_terminal()).init();
            let runtime = tokio::runtime::Runtime::new().context("the async runtime")?;
            runtime.block_on(async {
                let stop = bot::stop_signal()?;
                bot::run(cfg, |name| std::env::var(name).ok(), stop).await
            })
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
