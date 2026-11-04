//! The `trader` binary's command line.

use clap::Parser;

/// The monkeytrading bot.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {}

/// Parses the process's arguments; `--version` and `--help` print and exit.
pub fn run() {
    Cli::parse();
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;
    use clap::{CommandFactory, Parser};

    use crate::Cli;

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
}
