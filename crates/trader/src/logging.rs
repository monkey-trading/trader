//! The bot's log: the lines its level enables, with no secret in any of them. Each RPC endpoint's
//! URL, and each part of it that may carry a key, is replaced in every line as the line is written,
//! whoever formatted it and whatever the level's directives name; and the RPC client's own lines
//! below `info`, which print its URL, are not written at all. What the command line prints goes
//! through the same replacement.

use std::io::{self, Write};
use std::sync::Arc;

use anyhow::Context;
use tracing::{Level, Metadata, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use zeroize::Zeroizing;

use crate::config::Config;

/// Log targets, by prefix, whose lines below `info` are never written: below it, the RPC client
/// logs its endpoint's URL.
const QUIET: [&str; 4] = ["alloy_transport", "alloy_rpc_client", "reqwest", "hyper"];
/// The shortest part of a URL's path or query taken for a key on its own: shorter ones are names,
/// such as a network's, and the whole URL is replaced anyway.
const MIN_KEY_LEN: usize = 20;
/// What a secret is replaced with.
const REDACTED: &str = "[redacted]";

/// The strings no line may carry, longest first, so a part never replaces only some of a longer
/// secret.
#[derive(Clone, Default)]
pub struct Secrets(Arc<Vec<Zeroizing<String>>>);

impl Secrets {
    /// The secrets of `cfg`: its RPC endpoints' URLs.
    pub fn of(cfg: &Config) -> Self {
        Self::of_urls(cfg.chain.iter().flat_map(|chain| chain.rpc.iter().map(|url| url.expose())))
    }

    /// The secrets of `urls`: each as written and as parsed, its password, its query, and its user
    /// name, each path segment and each query value at least `MIN_KEY_LEN` long.
    pub fn of_urls<'a>(urls: impl IntoIterator<Item = &'a str>) -> Self {
        let mut secrets: Vec<Zeroizing<String>> = Vec::new();
        let mut add = |secret: &str| {
            if !secret.is_empty() && !secrets.iter().any(|known| known.as_str() == secret) {
                secrets.push(Zeroizing::new(secret.to_owned()));
            }
        };
        for text in urls {
            add(text);
            let Ok(url) = reqwest::Url::parse(text) else { continue };
            add(url.as_str());
            if url.username().len() >= MIN_KEY_LEN {
                add(url.username());
            }
            add(url.password().unwrap_or_default());
            add(url.query().unwrap_or_default());
            for segment in url.path_segments().into_iter().flatten().filter(|segment| segment.len() >= MIN_KEY_LEN) {
                add(segment);
            }
            for (_, value) in url.query_pairs().filter(|(_, value)| value.len() >= MIN_KEY_LEN) {
                add(value.as_ref());
            }
        }
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        Self(Arc::new(secrets))
    }

    /// `text` with every secret replaced.
    pub fn redact(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for secret in self.0.iter() {
            if text.contains(secret.as_str()) {
                text = text.replace(secret.as_str(), REDACTED);
            }
        }
        text
    }
}

/// A writer that writes each whole line to `inner` with every secret replaced, and what is left of
/// a line when flushed or dropped.
pub struct Redacting<W: Write> {
    inner: W,
    secrets: Secrets,
    line: Vec<u8>,
}

impl<W: Write> Redacting<W> {
    /// Writes to `inner` with `secrets` replaced.
    pub fn new(inner: W, secrets: Secrets) -> Self {
        Self { inner, secrets, line: Vec::new() }
    }

    /// Writes the first `len` bytes held, redacted.
    fn write_held(&mut self, len: usize) -> io::Result<()> {
        let redacted = self.secrets.redact(&String::from_utf8_lossy(&self.line[..len]));
        self.inner.write_all(redacted.as_bytes())?;
        self.line.drain(..len);
        Ok(())
    }
}

impl<W: Write> Write for Redacting<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.line.extend_from_slice(bytes);
        if let Some(end) = self.line.iter().rposition(|byte| *byte == b'\n') {
            self.write_held(end + 1)?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.line.is_empty() {
            self.write_held(self.line.len())?;
        }
        self.inner.flush()
    }
}

impl<W: Write> Drop for Redacting<W> {
    /// Writes what is left; a log line that cannot be written has nowhere else to go.
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

/// Makes a `Redacting` writer around each writer `inner` makes.
struct RedactingWriters<M> {
    inner: M,
    secrets: Secrets,
}

impl<'a, M: MakeWriter<'a>> MakeWriter<'a> for RedactingWriters<M> {
    type Writer = Redacting<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        Redacting::new(self.inner.make_writer(), self.secrets.clone())
    }
}

/// A subscriber writing the lines `level` enables to `writer` with `secrets` replaced, but no line
/// below `info` from a `QUIET` target, which no directive in `level` can enable.
/// Error cases:
/// - `level` is not a filter
pub fn subscriber<W>(
    level: &str,
    ansi: bool,
    writer: W,
    secrets: Secrets,
) -> anyhow::Result<impl Subscriber + Send + Sync + 'static>
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    let filter = EnvFilter::try_new(level).context("logging.level")?;
    let writer = RedactingWriters { inner: writer, secrets };
    let fmt = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(ansi).with_writer(writer).finish();
    Ok(fmt.with(filter_fn(audible)))
}

/// Whether a line may be written: one at `info` or above, or from a target outside `QUIET`.
fn audible(metadata: &Metadata<'_>) -> bool {
    *metadata.level() <= Level::INFO || !QUIET.iter().any(|quiet| metadata.target().starts_with(quiet))
}

#[cfg(test)]
mod tests;
