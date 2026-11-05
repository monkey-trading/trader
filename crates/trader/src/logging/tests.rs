use std::io::Write;
use std::sync::{Arc, Mutex};

use super::*;

/// A key as a provider puts it in an endpoint's path or query.
const KEY: &str = "k3y0f32charactersfromaprovider00";

/// What a test's subscriber wrote.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<u8>>>);

impl Write for Lines {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Lines {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

/// An endpoint's URL, its key in the path or the query, and its credentials are each a secret;
/// a short path segment, such as a network's name, is not on its own.
#[test]
fn every_part_of_a_url_that_may_carry_a_key_is_replaced() {
    let urls = [
        format!("https://arbitrum.example/v2/{KEY}"),
        format!("https://rpc.example/arbitrum?apikey={KEY}&chain=arbitrum"),
        "https://user:hunter2-password@rpc.example/".to_owned(),
    ];
    let secrets = Secrets::of_urls(urls.iter().map(String::as_str));
    for url in &urls {
        assert_eq!(secrets.redact(&format!("sending to {url}")), format!("sending to {REDACTED}"));
    }
    let pieces = secrets.redact(&format!("key {KEY}, query apikey={KEY}&chain=arbitrum, password hunter2-password"));
    assert_eq!(pieces, format!("key {REDACTED}, query {REDACTED}, password {REDACTED}"));
    assert_eq!(secrets.redact("the arbitrum network, v2"), "the arbitrum network, v2");
}

/// Whatever the level's directives name, the RPC client's lines below `info` are not written, and a
/// secret in any line that is written, from any target, is replaced.
#[test]
fn no_secret_is_written_whatever_the_level_names() {
    let url = format!("https://arbitrum.example/v2/{KEY}");
    let lines = Lines::default();
    let writer = lines.clone();
    let level = "trace,alloy_transport_http::reqwest_transport=trace,reqwest::connect=trace,hyper_util=trace";
    let subscriber = subscriber(level, false, move || writer.clone(), Secrets::of_urls([url.as_str()])).unwrap();
    tracing::subscriber::with_default(subscriber, || {
        tracing::debug!(target: "alloy_transport_http::reqwest_transport", url = %url, "sending");
        tracing::trace!(target: "reqwest::connect", "{url}");
        tracing::debug!(target: "hyper_util::client::legacy", "{url}");
        tracing::info!(target: "reqwest::connect", "the client's info line: {url}");
        tracing::warn!(target: "trader::chain", "the bot's own line: {KEY}");
    });
    let written = lines.text();
    assert!(!written.contains(KEY), "{written}");
    assert!(written.contains("the client's info line: [redacted]"), "{written}");
    assert!(written.contains("the bot's own line: [redacted]"), "{written}");
    assert_eq!(written.lines().count(), 2, "{written}");
}

/// A writer holds a line written in pieces until it ends, so a secret split across writes is still
/// replaced, and writes what is left when flushed.
#[test]
fn lines_written_in_pieces_are_replaced_whole() {
    let lines = Lines::default();
    let mut out = Redacting::new(lines.clone(), Secrets::of_urls([format!("https://rpc.example/{KEY}").as_str()]));
    write!(out, "first {}", &KEY[..10]).unwrap();
    writeln!(out, "{} done", &KEY[10..]).unwrap();
    write!(out, "then {KEY}").unwrap();
    assert_eq!(lines.text(), format!("first {REDACTED} done\n"));
    out.flush().unwrap();
    assert_eq!(lines.text(), format!("first {REDACTED} done\nthen {REDACTED}"));
}
