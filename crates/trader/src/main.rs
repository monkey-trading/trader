//! Entry point of the `trader` binary.

use std::process::ExitCode;

/// Runs the bot, and exits non-zero with the reason when it fails.
fn main() -> ExitCode {
    match trader::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
