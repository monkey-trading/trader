//! What a transaction has come to while a call that sends it is still running, for a caller that
//! shows it: the task running the call names a sink, and `send` reports to it as each transaction is
//! signed and sent, and as it is mined, reverts or is replaced. A call with no sink reports nowhere.

use std::future::Future;
use std::sync::Arc;

use alloy_primitives::B256;

/// A step of a transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Signed and sent, or taken by an endpoint already: what it does, for the caller's display.
    Sent { tx: B256, what: String },
    /// Mined, or found landed by its effect.
    Mined { tx: B256 },
    /// Mined and reverted.
    Reverted { tx: B256 },
    /// Never to be mined: another transaction took its nonce, and the call is signed again or fails.
    Replaced { tx: B256 },
}

/// Where a task's transactions report.
pub type Sink = Arc<dyn Fn(Progress) + Send + Sync>;

tokio::task_local! {
    static SINK: Sink;
}

/// Runs `work` with `sink` receiving the progress of every transaction it sends.
pub async fn with_sink<F: Future>(sink: Sink, work: F) -> F::Output {
    SINK.scope(sink, work).await
}

/// Reports `progress` to the running task's sink, if it has one.
pub(super) fn report(progress: Progress) {
    // A task with no sink has nobody to tell.
    let _ = SINK.try_with(|sink| sink(progress));
}
