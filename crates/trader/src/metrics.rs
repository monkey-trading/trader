//! The bot's Prometheus metrics, served in OpenMetrics text on a loopback listener (`[metrics]`):
//! orders placed and declined and fills, per account and market; each account's equity and margin
//! level; and the stream's lag. The registry belongs to the process: `serve` builds it and hands it
//! to the bot and the listener. The fleet holds each series it records (`FleetMetrics`), so recording
//! one is an atomic operation that allocates nothing; registering a label set takes a lock that
//! encoding also takes.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::anyhow;
use axum::Router;
use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use axum::routing::get;
use engine_core::Fixed;
use engine_core::event::Event;
use engine_core::fixed::DECIMALS;
use engine_core::types::{AccountId, MarketId, Millis};
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::encoding::text::encode;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::{Registry, Unit};
use tokio::net::TcpListener;

/// The OpenMetrics text format's content type.
const CONTENT_TYPE_OPENMETRICS: &str = "application/openmetrics-text; version=1.0.0; charset=utf-8";

/// A gauge of a fractional value.
pub type FloatGauge = Gauge<f64, AtomicU64>;

/// The process's metrics.
pub struct Metrics {
    registry: Registry,
    orders_placed: Family<AccountMarket, Counter>,
    orders_declined: Family<AccountMarket, Counter>,
    fills: Family<AccountMarket, Counter>,
    equity: Family<Account, FloatGauge>,
    margin_level: Family<Account, FloatGauge>,
    stream_lag: FloatGauge,
}

/// One account's series in one market.
#[derive(Clone, Default)]
pub struct MarketMetrics {
    /// Orders and closes signed and sent to the node, each counted in every market it names.
    pub orders_placed: Counter,
    /// Those the node refused in its answer, or the engine rejected in the log.
    pub orders_declined: Counter,
    /// The account's fills in the market, as the log shows them.
    pub fills: Counter,
}

/// One account's equity and margin level.
#[derive(Clone, Default)]
pub struct AccountMetrics {
    equity: FloatGauge,
    margin_level: FloatGauge,
}

/// The series a fleet records: each assignment's (an account in a market) and each account's, by
/// their places in the fleet, and the stream's lag. A fleet built without them records nothing.
#[derive(Default)]
pub struct FleetMetrics {
    pub markets: Vec<MarketMetrics>,
    pub accounts: Vec<AccountMetrics>,
    pub stream_lag: FloatGauge,
}

impl FleetMetrics {
    /// Clears the figures of the fleet's account `account`, which has failed: they are no longer read.
    pub fn account_failed(&self, account: u32) {
        if let Some(series) = self.accounts.get(account as usize) {
            series.clear();
        }
    }
}

/// One account's order series, by market, for the requests it signs: a close of several markets
/// counts in each.
#[derive(Default)]
pub struct OrderSeries(pub Vec<(MarketId, MarketMetrics)>);

impl OrderSeries {
    /// Counts a request naming `markets` as placed in each.
    pub fn placed(&self, markets: &[MarketId]) {
        for series in self.of(markets) {
            series.orders_placed.inc();
        }
    }

    /// Counts a request naming `markets` as declined in each: the node refused it whole.
    pub fn declined(&self, markets: &[MarketId]) {
        for series in self.of(markets) {
            series.orders_declined.inc();
        }
    }

    /// Counts each rejection among `events`, the engine's answer to a request naming `markets`, in the
    /// market it names: an order's own, or the close its `client_id` indexes, the first for a close
    /// the engine refused whole.
    pub fn rejected(&self, markets: &[MarketId], events: &[Event]) {
        for event in events {
            if let Event::OrderRejected { client_id, .. } = event {
                let market = usize::try_from(*client_id).ok().and_then(|i| markets.get(i).or(markets.first())).copied();
                self.declined(market.as_slice());
            }
        }
    }

    fn of<'a>(&'a self, markets: &'a [MarketId]) -> impl Iterator<Item = &'a MarketMetrics> {
        self.0.iter().filter(|(market, _)| markets.contains(market)).map(|(_, series)| series)
    }
}

/// The labels of an account's series in a market.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct AccountMarket {
    account: String,
    market: String,
}

/// The label of an account's series.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct Account {
    account: String,
}

impl Metrics {
    /// An empty registry with every metric the bot records.
    pub fn new() -> Self {
        let mut registry = Registry::with_prefix("trader");
        let orders_placed = Family::<AccountMarket, Counter>::default();
        let orders_declined = Family::<AccountMarket, Counter>::default();
        let fills = Family::<AccountMarket, Counter>::default();
        let equity = Family::<Account, FloatGauge>::default();
        let margin_level = Family::<Account, FloatGauge>::default();
        let stream_lag = FloatGauge::default();
        registry.register("orders_placed", "Orders signed and sent to the node", orders_placed.clone());
        registry.register("orders_declined", "Orders the node or the engine refused", orders_declined.clone());
        registry.register("fills", "The account's fills in the market, from the log", fills.clone());
        registry.register(
            "account_equity_usd",
            "The account's cash and its positions' profit or loss at their marks",
            equity.clone(),
        );
        registry.register(
            "account_margin_level",
            "The account's equity over its margin requirement; the engine closes positions below 0.5",
            margin_level.clone(),
        );
        registry.register_with_unit(
            "stream_lag",
            "The bot's clock when it reads a command from the log, less the time the node sequenced it",
            Unit::Seconds,
            stream_lag.clone(),
        );
        Self { registry, orders_placed, orders_declined, fills, equity, margin_level, stream_lag }
    }

    /// `account`'s series in `market`, by its symbol.
    pub fn market(&self, account: AccountId, market: &str) -> MarketMetrics {
        let labels = AccountMarket { account: account.to_string(), market: market.to_owned() };
        MarketMetrics {
            orders_placed: self.orders_placed.get_or_create_owned(&labels),
            orders_declined: self.orders_declined.get_or_create_owned(&labels),
            fills: self.fills.get_or_create_owned(&labels),
        }
    }

    /// `account`'s equity and margin level, each not a number until the account is first valued, so
    /// that no threshold on either fires before then.
    pub fn account(&self, account: AccountId) -> AccountMetrics {
        let labels = Account { account: account.to_string() };
        let series = AccountMetrics {
            equity: self.equity.get_or_create_owned(&labels),
            margin_level: self.margin_level.get_or_create_owned(&labels),
        };
        series.clear();
        series
    }

    /// The stream's lag, for the task that reads the log.
    pub fn stream_lag(&self) -> FloatGauge {
        self.stream_lag.clone()
    }

    /// The registry in OpenMetrics text format.
    pub fn encode(&self) -> String {
        let mut text = String::new();
        // Writing into a `String` cannot fail.
        encode(&mut text, &self.registry).expect("metrics encode into a String");
        text
    }
}

impl Default for Metrics {
    /// See `new`.
    fn default() -> Self {
        Self::new()
    }
}

impl AccountMetrics {
    /// Sets both figures to not a number: the bot has none for the account, before it is first valued
    /// or once it has failed, and no threshold on a figure it no longer has may fire or hold.
    pub fn clear(&self) {
        self.equity.set(f64::NAN);
        self.margin_level.set(f64::NAN);
    }

    /// Records the account's `equity` and margin `requirement`, in USD: the margin level is their
    /// ratio, not a number while nothing is required, so no threshold on it fires.
    pub fn set_equity(&self, equity: Fixed, requirement: Fixed) {
        self.equity.set(usd(equity));
        let level = if requirement.is_zero() { f64::NAN } else { usd(equity) / usd(requirement) };
        self.margin_level.set(level);
    }
}

/// How long after `ts`, a sequenced time in milliseconds, the clock read `now`, in seconds: a float
/// for reading only, negative when this machine's clock is behind the node's.
pub fn lag_seconds(now: SystemTime, ts: Millis) -> f64 {
    let now_ms = now.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis());
    (now_ms as f64 - ts as f64) / 1000.0
}

/// Serves `metrics` at `GET /metrics` on `listener` until the listener fails, and says why.
pub async fn serve(listener: TcpListener, metrics: Arc<Metrics>) -> anyhow::Error {
    let router = Router::new().route("/metrics", get(scrape)).with_state(metrics);
    match axum::serve(listener, router).await {
        Ok(()) => anyhow!("the metrics listener closed"),
        Err(e) => e.into(),
    }
}

/// The current metrics.
async fn scrape(State(metrics): State<Arc<Metrics>>) -> impl IntoResponse {
    ([(CONTENT_TYPE, CONTENT_TYPE_OPENMETRICS)], metrics.encode())
}

/// `amount` in USD as a float: a gauge's value is for reading, and nothing is computed from it.
fn usd(amount: Fixed) -> f64 {
    amount.raw() as f64 / 10f64.powi(DECIMALS.cast_signed())
}

#[cfg(test)]
mod tests;
