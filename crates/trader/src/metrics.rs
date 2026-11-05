//! The bot's Prometheus metrics, served in OpenMetrics text on a loopback listener (`[metrics]`):
//! orders placed and declined and fills, per account and market; each account's equity and margin
//! level; and the stream's lag. The registry belongs to the process: `run` builds it and hands it
//! to the bot and the listener. Each account's task holds its own series, so recording one is an
//! atomic operation that allocates nothing; registering a label set takes a lock that encoding
//! also takes.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use anyhow::anyhow;
use axum::Router;
use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use axum::routing::get;
use engine_core::Fixed;
use engine_core::fixed::DECIMALS;
use engine_core::types::AccountId;
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

/// One account's series in one market, held by the task that trades it.
#[derive(Clone)]
pub struct AccountMetrics {
    /// Orders signed and sent to the node.
    pub orders_placed: Counter,
    /// Orders the node or the engine refused.
    pub orders_declined: Counter,
    /// The account's fills in the market, as the log shows them.
    pub fills: Counter,
    equity: FloatGauge,
    margin_level: FloatGauge,
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

    /// `account`'s series in `market`, by its symbol, registered with the account's.
    pub fn account(&self, account: AccountId, market: &str) -> AccountMetrics {
        let labels = AccountMarket { account: account.to_string(), market: market.to_owned() };
        let account = Account { account: labels.account.clone() };
        AccountMetrics {
            orders_placed: self.orders_placed.get_or_create_owned(&labels),
            orders_declined: self.orders_declined.get_or_create_owned(&labels),
            fills: self.fills.get_or_create_owned(&labels),
            equity: self.equity.get_or_create_owned(&account),
            margin_level: self.margin_level.get_or_create_owned(&account),
        }
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
    /// Records the account's `equity` and margin `requirement`, in USD: the margin level is their
    /// ratio, not a number while nothing is required, so no threshold on it fires.
    pub fn set_equity(&self, equity: Fixed, requirement: Fixed) {
        self.equity.set(usd(equity));
        let level = if requirement.is_zero() { f64::NAN } else { usd(equity) / usd(requirement) };
        self.margin_level.set(level);
    }
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
