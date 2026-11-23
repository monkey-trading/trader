//! What a stop asks of the node and of its own state before it signs: whether the account halted, the
//! expiry to sign with, the account and its next nonce.

use engine_core::types::{AccountId, Millis};
use std::sync::atomic::Ordering;
use tracing::warn;

use crate::bot::fleet::EXPIRES_IN_MS;
use crate::bot::stop::{Failed, Stop};
use crate::node::CallError;

impl Stop {
    /// Ends the stop of an account that has stopped trading since it began, before anything more is signed.
    /// Error cases:
    /// - the account halted
    pub(super) fn check_halt(&self) -> Result<(), Failed> {
        match &self.halt {
            Some(halt) if halt.load(Ordering::Relaxed) => {
                Err(Failed::Incomplete("the account halted: nothing more was signed for it".into()))
            }
            _ => Ok(()),
        }
    }

    /// The expiry for a request signed now: the node's time as the bot last verified it, counted on,
    /// plus the window. `None`, with a warning, before any answer.
    pub(super) fn expiry(&self) -> Option<Millis> {
        let now = self.node.latest_time_ms().and_then(|ms| Millis::try_from(ms).ok());
        if now.is_none() {
            warn!("the node's time is not known: nothing is signed");
        }
        now.map(|now| now.saturating_add(EXPIRES_IN_MS))
    }

    /// The account, verified.
    pub(super) async fn read(&self, account: AccountId) -> Result<crate::node::api::AccountView, Failed> {
        match self.node.account(account).await {
            Ok(answer) => Ok(answer.body),
            Err(CallError::WrongSigner { .. }) => Err(Failed::WrongSigner),
            Err(e) => Err(Failed::Incomplete(e.to_string())),
        }
    }

    /// The account's next nonce, or `None`, with a warning, when the node gives no answer to rely on.
    pub(super) async fn nonce(&self, account: AccountId) -> Result<Option<u64>, Failed> {
        match self.node.nonce(account).await {
            Ok(answer) => Ok(Some(answer.body.next_nonce)),
            Err(CallError::WrongSigner { .. }) => Err(Failed::WrongSigner),
            Err(e) => {
                warn!(%account, "no nonce to sign with: {e}");
                Ok(None)
            }
        }
    }
}
