//! The README's quick start against what the bot's stop takes.

use std::path::Path;

/// The seconds the stop waits for the log, at most, before its requests.
const LOG_WAIT: u64 = 45;
/// The requests a stop sends for an account: reads, a nonce, a cancel and a close.
const REQUESTS: u64 = 6;
/// The fleet the quick start sizes its timeouts for, and the node's default `per_ip` rate.
const ACCOUNTS: u64 = 1_000;
const PER_SECOND: u64 = 50;

/// The Docker and systemd examples give the stop the time a fleet's stop takes under the node's
/// default limits, so the kill they set is not one in the middle of it.
#[test]
fn the_quick_starts_stop_timeouts_fit_a_fleets_stop() {
    let readme = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md")).unwrap();
    let needed = LOG_WAIT + ACCOUNTS * REQUESTS / PER_SECOND;
    for (name, marker) in [("Docker", "--stop-timeout "), ("systemd", "TimeoutStopSec=")] {
        let at = readme.find(marker).unwrap_or_else(|| panic!("no {name} timeout in the README")) + marker.len();
        let digits: String = readme[at..].chars().take_while(char::is_ascii_digit).collect();
        let seconds: u64 = digits.parse().unwrap();
        assert!(
            seconds >= needed,
            "the {name} example gives the stop {seconds} s, and {ACCOUNTS} accounts need {needed}"
        );
    }
}
