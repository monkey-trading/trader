//! `trader accounts`: the lifecycle actions for the config's accounts, one account after another,
//! each account's outcome printed as it ends. One account's failure is printed and the others carry
//! on; the command fails if any did.

use std::io::{BufRead, Write};

use alloy_primitives::U256;
use alloy_primitives::utils::format_units;
use anyhow::{Context, bail};
use clap::{Subcommand, ValueEnum};
use engine_core::Fixed;
use engine_core::access::EligibilityClass;
use engine_core::types::AccountId;
use futures_util::future::join_all;
use tokio::task::spawn_blocking;

use crate::accounts::claim::Claimed;
use crate::accounts::declare::{Confirmed, Declared};
use crate::accounts::deposit::{Deposited, Source};
use crate::accounts::{AccountStatus, Lifecycle, USDC_DECIMALS};
use crate::config::{Config, MONKEY_DECIMALS};
use crate::keys::AccountKey;

/// How many keystores are decrypted at once: key derivation is slow by design.
const UNLOCK_AT_ONCE: usize = 8;

/// What `trader accounts` does for each account.
#[derive(Debug, Subcommand)]
pub enum Action {
    /// Shows each account's declaration, wallet, balance, unclaimed MONKEY and withdrawals.
    Status,
    /// Shows the declaration and records it for each account the user confirms.
    Declare {
        /// The declaration's class, which the user states.
        #[arg(long, value_enum)]
        class: Class,
        /// Confirms the declaration for this account without asking; repeat for each account.
        #[arg(long = "confirm", value_name = "ADDRESS")]
        confirm: Vec<AccountId>,
    },
    /// Deposits USDC into each account from its own wallet, and waits for the credit.
    Deposit {
        /// The USDC each account receives.
        #[arg(long)]
        amount: Fixed,
        /// Deposits from the funding key's wallet instead, with `depositFor`.
        #[arg(long)]
        from_funding: bool,
    },
    /// Claims each account's unclaimed MONKEY to the account.
    Claim,
    /// Withdraws USDC from each account to its own address, and pays its approved withdrawals.
    Withdraw {
        /// The USDC to withdraw; without it, only approved withdrawals are paid.
        #[arg(long)]
        amount: Option<Fixed>,
    },
}

/// A declaration's class, as the command line names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Class {
    Professional,
    EligibleClient,
}

impl From<Class> for EligibilityClass {
    fn from(class: Class) -> Self {
        match class {
            Class::Professional => Self::Professional,
            Class::EligibleClient => Self::EligibleClient,
        }
    }
}

/// Runs `action` for the accounts of `cfg` that `only` names, or all of them, unlocking keys with
/// `env`, and writes each account's outcome to `out`; a declaration asks the user to confirm when
/// `terminal`, as when standard input is one.
/// Error cases:
/// - `only` or a confirmation names an account the config does not
/// - a key does not load, or the lifecycle cannot connect (see `Lifecycle::connect`)
/// - the action fails for any account
pub async fn run(
    cfg: Config,
    only: Vec<AccountId>,
    action: Action,
    env: impl Fn(&str) -> Option<String>,
    terminal: bool,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut keys: Vec<AccountKey> = Vec::with_capacity(cfg.plans.len());
    for chunk in cfg.plans.chunks(UNLOCK_AT_ONCE) {
        for key in join_all(chunk.iter().map(|plan| AccountKey::load(&plan.key, &env))).await {
            let key = key?;
            if !keys.iter().any(|known| known.address() == key.address()) {
                keys.push(key);
            }
        }
    }
    if let Some(unknown) = only.iter().find(|account| !keys.iter().any(|key| key.address() == **account)) {
        bail!("the config has no account {unknown}");
    }
    keys.retain(|key| only.is_empty() || only.contains(&key.address()));
    let lifecycle = Lifecycle::connect(&cfg, &env).await?;
    let confirmed = match &action {
        Action::Declare { class, confirm } => {
            confirm_declarations(&lifecycle, &keys, (*class).into(), confirm, terminal, out).await?
        }
        _ => Vec::new(),
    };
    let mut failed = 0;
    for key in &keys {
        let account = key.address();
        let outcome = match &action {
            Action::Status => lifecycle.status(account).await.map(|status| describe_status(&status)),
            Action::Declare { .. } => match confirmed.iter().find(|confirmed| confirmed.account == account) {
                Some(confirmed) => lifecycle.declare(key, *confirmed).await.map(|declared| match declared {
                    Declared::Already(class) => format!("declared {class:?} already"),
                    Declared::Recorded(class) => format!("declared {class:?}"),
                    Declared::Refused(reason) => format!("declaration refused by the node: {reason}"),
                }),
                None => lifecycle.declared(account).await.map(|declared| match declared {
                    Some(class) => format!("declared {class:?} already"),
                    None => String::from("not declared: not confirmed"),
                }),
            },
            Action::Deposit { amount, from_funding } => {
                let source = if *from_funding { Source::Funding } else { Source::Account };
                lifecycle.deposit(key, *amount, source).await.map(|deposited| match deposited {
                    Deposited::Credited { tx } => format!("deposited {amount} USDC in {tx}, credited"),
                    Deposited::AwaitingDeclaration { tx } => {
                        format!("deposited {amount} USDC in {tx}; it credits once the account declares")
                    }
                })
            }
            Action::Claim => lifecycle.claim(key, U256::from(1)).await.map(|claimed| match claimed {
                Claimed::Nothing => String::from("no MONKEY entitlement yet"),
                Claimed::Waiting { unclaimed } => {
                    format!(
                        "{} MONKEY becomes claimable once the vault records the newest checkpoint",
                        monkey(unclaimed)
                    )
                }
                Claimed::Below { .. } => String::from("no MONKEY to claim"),
                Claimed::Minted { amount, tx } => format!("claimed {} MONKEY in {tx}", monkey(amount)),
            }),
            Action::Withdraw { amount } => lifecycle.withdraw(key, *amount).await.map(|paid| {
                let paid: Vec<String> =
                    paid.iter().map(|paid| format!("{} USDC in {}", paid.amount, paid.tx)).collect();
                if paid.is_empty() { String::from("no withdrawal to pay") } else { format!("paid {}", paid.join(", ")) }
            }),
        };
        match outcome {
            Ok(line) => writeln!(out, "{account}: {line}")?,
            Err(e) => {
                writeln!(out, "{account}: failed: {e:#}")?;
                failed += 1;
            }
        }
    }
    if failed > 0 {
        bail!("{failed} of {} accounts failed", keys.len());
    }
    Ok(())
}

/// The confirmed declarations of `class` for `keys`' accounts that have not declared: those
/// `confirm` names, and, in a `terminal`, those the user confirms when asked after reading the
/// statement.
/// Error cases:
/// - `confirm` names an account not among `keys`
/// - a declaration cannot be read, or the terminal cannot be read
async fn confirm_declarations(
    lifecycle: &Lifecycle,
    keys: &[AccountKey],
    class: EligibilityClass,
    confirm: &[AccountId],
    terminal: bool,
    out: &mut impl Write,
) -> anyhow::Result<Vec<Confirmed>> {
    if let Some(unknown) = confirm.iter().find(|account| !keys.iter().any(|key| key.address() == **account)) {
        bail!("--confirm names {unknown}, which is not one of the accounts");
    }
    let mut undeclared = Vec::new();
    for key in keys {
        if lifecycle.declared(key.address()).await?.is_none() {
            undeclared.push(key.address());
        }
    }
    if undeclared.is_empty() {
        return Ok(Vec::new());
    }
    writeln!(out, "Each account confirmed below signs this declaration, which the node records in its public log:")?;
    writeln!(out, "\n    {}\n", class.statement())?;
    out.flush()?;
    let mut confirmed = Vec::new();
    for account in undeclared {
        let yes = if confirm.contains(&account) {
            true
        } else if terminal {
            ask(format!("Declare for {account}? Type yes to confirm: ")).await?
        } else {
            writeln!(out, "{account}: not confirmed; pass --confirm {account} or run in a terminal")?;
            false
        };
        if yes {
            confirmed.push(Confirmed { account, class });
        }
    }
    Ok(confirmed)
}

/// Asks `question` in the terminal; whether the answer is `yes`.
/// Error cases:
/// - the terminal cannot be written or read
async fn ask(question: String) -> anyhow::Result<bool> {
    spawn_blocking(move || {
        print!("{question}");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        anyhow::Ok(answer.trim().eq_ignore_ascii_case("yes"))
    })
    .await
    .context("the terminal")?
}

/// One line of an account's status.
fn describe_status(status: &AccountStatus) -> String {
    let declared = status.declared.map_or_else(|| String::from("not declared"), |class| format!("declared {class:?}"));
    let balance = status.balance.map_or_else(|| String::from("never credited"), |balance| format!("{balance} USDC"));
    let queued: Vec<String> = status
        .queued
        .iter()
        .map(|queued| format!("{} USDC at position {}", queued.remaining, queued.position))
        .collect();
    let unpaid: Vec<String> = status
        .unpaid
        .iter()
        .map(|payout| {
            let state = if payout.withdrawal.is_some() { "authorized" } else { "awaiting authorization" };
            format!("{} USDC ({state})", payout.amount)
        })
        .collect();
    let line = format!(
        "{declared}; wallet {} ETH, {} USDC, {} MONKEY; balance {balance}; unclaimed MONKEY {} ({} at the newest checkpoint); queued [{}]; unpaid [{}]",
        units(status.wallet_eth, 18),
        units(status.wallet_usdc, USDC_DECIMALS),
        monkey(status.wallet_monkey),
        monkey(status.unclaimed),
        monkey(status.unclaimed_newest),
        queued.join(", "),
        unpaid.join(", "),
    );
    if status.payouts_loaded {
        line
    } else {
        format!("{line} (the node is still loading payouts, so some may be missing)")
    }
}

/// MONKEY base units as MONKEY.
fn monkey(units_of: U256) -> String {
    units(units_of, MONKEY_DECIMALS)
}

/// `value` base units of a token with `decimals`, as a decimal without trailing zeros.
fn units(value: U256, decimals: u8) -> String {
    let text = format_units(value, decimals).expect("decimals within range");
    match text.split_once('.') {
        Some((whole, fraction)) => {
            let fraction = fraction.trim_end_matches('0');
            if fraction.is_empty() { whole.to_owned() } else { format!("{whole}.{fraction}") }
        }
        None => text,
    }
}
