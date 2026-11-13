//! An account's declaration, signed by its key and recorded by the node (`POST /v1/eligibility`),
//! only for an account whose user has read the statement and confirmed it.

use anyhow::{bail, ensure};
use engine_core::access::{EligibilityClass, EligibilityRequest};
use engine_core::auth::RequestAuth;
use engine_core::eip712::eligibility_hash;
use engine_core::event::Event;
use engine_core::types::AccountId;
use tokio::time::{Instant, sleep};
use tracing::{Instrument, info, info_span, warn};

use crate::accounts::{Lifecycle, REQUEST_EXPIRY_MS, retry_later};
use crate::keys::AccountKey;
use crate::node::CallError;

/// The user's confirmation that `account` makes the declaration of `class`, given once they have
/// read its statement (`EligibilityClass::statement`). The bot declares for no account without one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Confirmed {
    pub account: AccountId,
    pub class: EligibilityClass,
}

/// What a declaration came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Declared {
    /// The account had already declared this class.
    Already(EligibilityClass),
    /// The node recorded it.
    Recorded(EligibilityClass),
    /// The node refused it in the log, for this reason; the account may declare again.
    Refused(String),
}

impl Lifecycle {
    /// Records `confirmed`'s declaration, signed by `key`, unless the account has declared already.
    /// Each request waits its turn under the node's fresh-account budget for this client
    /// (`node.limits.fresh_accounts`); a retry-later answer, as those budgets give, is waited out and the
    /// declaration asked again, as is a stale nonce; an answer that never came is checked against
    /// the account's recorded declaration before asking again.
    /// Error cases:
    /// - `confirmed` is for another account than `key`'s
    /// - the account declared another class already
    /// - the node refuses it outside the log, or nothing is recorded within `settle_timeout`
    pub async fn declare(&self, key: &AccountKey, confirmed: Confirmed) -> anyhow::Result<Declared> {
        let account = key.address();
        ensure!(confirmed.account == account, "the confirmation is for {}, not {account}", confirmed.account);
        let span = info_span!("declare", id = %account);
        async {
            let deadline = Instant::now() + self.settle_timeout;
            let mut asked = false;
            loop {
                match self.node.eligibility(account).await?.body.class {
                    Some(class) if class == confirmed.class => {
                        return Ok(if asked { Declared::Recorded(class) } else { Declared::Already(class) });
                    }
                    Some(class) => bail!("{account} declared {class:?} already"),
                    None => {}
                }
                if Instant::now() >= deadline {
                    bail!("{account}'s declaration was not recorded within {:?}", self.settle_timeout);
                }
                // Its turn comes before it is signed, so the wait never runs into its expiry.
                self.node.setup_turn().await;
                let (nonce, now) = self.next_nonce(account).await?;
                let expires_at = now.saturating_add(REQUEST_EXPIRY_MS);
                let hash = eligibility_hash(&self.domain, account, confirmed.class, nonce, expires_at);
                let auth = RequestAuth { nonce, expires_at, signature: key.sign(&hash) };
                let request = EligibilityRequest { account, class: confirmed.class, auth };
                info!(class = ?confirmed.class, nonce, expires_at, "declaring");
                asked = true;
                let error = match self.node.declare(&request).await {
                    Ok(answer) => return outcome(&answer.body),
                    Err(error) => error,
                };
                match (retry_later(&error), &error) {
                    (Some(wait), _) => {
                        info!(?wait, "the node asks to declare later: {error}");
                        sleep(wait.min(deadline.saturating_duration_since(Instant::now()))).await;
                    }
                    (None, CallError::Refused { status: 409, .. }) => info!("the nonce moved on; signing again"),
                    (None, CallError::Refused { .. } | CallError::WrongSigner { .. }) => return Err(error.into()),
                    (None, _) => {
                        warn!("no answer to the declaration; reading the account again: {error}");
                        sleep(self.poll_interval).await;
                    }
                }
            }
        }
        .instrument(span)
        .await
    }
}

/// What the node's events for a declaration say.
/// Error cases:
/// - the events neither record nor refuse a declaration
fn outcome(events: &[Event]) -> anyhow::Result<Declared> {
    for event in events {
        match event {
            Event::EligibilityAttested { class, .. } => {
                info!(?class, "declaration recorded");
                return Ok(Declared::Recorded(*class));
            }
            Event::EligibilityRejected { reason, .. } => {
                warn!(?reason, "declaration refused");
                return Ok(Declared::Refused(format!("{reason:?}")));
            }
            _ => {}
        }
    }
    bail!("the node's answer neither records nor refuses the declaration: {events:?}")
}
