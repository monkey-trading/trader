# How the bot works

This page describes what the bot does from the moment it starts until it stops: how it checks the
trading server, how it turns the server's public log into bars and orders, how it paces its
requests, and what it does when it stops or meets something it cannot follow. It also describes the
`trader accounts` commands that fund an account and withdraw from it. The
[README](../README.md) shows how to run the bot, [Configuration](configuration.md) documents every
setting, and [The console](console.md) describes the web console.

The bot trades against a trading server built from the engine repository. The engine's code calls
that server a node, and the bot's configuration and logs use the same name.

## Verifying the trading server

The trading server runs inside an enclave, a virtual machine whose memory the host machine can
neither read nor alter. The enclave's processor signs an attestation, a report that identifies the
exact code the enclave runs, the key it signs with, and the key its TLS connections present.

Before it makes any other call, the bot reads the server's attestation and verifies it with the
engine's own `engine-verify` code, at the engine commit the bot pins. The report's certificate
chain must end at a vendor root compiled into the bot, and its launch measurement must match one of
the release manifests that the config names. A development build can instead accept a development
server's simulated attestation, which proves nothing. If the attestation does not verify, the bot
raises a Critical alert and stops before it reads anything else.

The enclave address that the attestation names is the only signer the bot accepts, and the TLS key
it names is the only key that the bot's connections accept from then on. The signer is never taken
from the config.

- Every REST call goes over TLS 1.3 to a server that holds the attested key. Each call carries a
  fresh random challenge, and the bot verifies the signed answer with the engine's `engine-wire`
  code.
- The bot reads the public log over one signed stream connection. It verifies every message with
  `engine-wire`, refuses a gap in the sequence, and resumes after the last message it read when the
  connection fails or falls silent.
- An answer that fails its check is treated as an error, never as data. An answer or stream message
  signed by another key stops the bot, as does a stream message that the bot cannot read.

## Many accounts on one stream

One process trades every account in the config, each on its own key, over one stream connection.
The bot reads the log once. Each price update moves its market's reference price once, and each
combination of market and bar length has one series of bars. A strategy with one set of
parameters runs once on a series, however many accounts trade it. When a bar completes, each
account that reads it receives a target position, which the account's controls turn into an order
or into no order.

When it starts, the bot reads the markets and each account's positions and balance. From then on,
each position follows its account's executions in the log. The bot reads the account again after
its executions, before each decision to trade, and every minute.

- **Someone else trading the account.** The bot recognises its own executions by the signatures it
  made for that account. Once the orders signed before the process started have expired, an
  execution from an order that this process did not sign means that someone else is trading the
  account. The bot then stops that account, and reports `account_shared`. The other accounts carry
  on.
- **An account that cannot start.** The bot reports `account_failed` for the account, and stops
  that account alone, in any of these cases:
  - the account's key does not unlock;
  - the server has never credited the account, or its balance is empty;
  - the account holds a market that the bot cannot follow;
  - the server refuses the account.

  The other accounts carry on, and the run exits with a non-zero status when it ends.

## Requests, rate limits and nonces

The bot's requests share a small pool of HTTP/1.1 connections that it keeps open, and are paced
under the server's rate limits, which `[node.limits]` sets. When a burst of orders exceeds the
limits, the orders wait in the order in which their bars completed, rather than failing. Each order
is signed only when the limits let it go. An answer that asks the bot to retry later, with HTTP
status 429 or 503, holds every request back for the delay it names. A read is then sent again, but
an order is not.

Every signed request carries the account's next nonce, a counter that the server accepts once.

- Each account tracks its own nonce, and moves it on after an order that the engine used.
- After any refusal, rejection, or order that received no answer, the bot reads the account's
  nonce again from the server.
- Each account has at most one request outstanding at a time, so its nonces reach the server in
  order.
- An order that the server refuses for its nonce never entered the log, for example because a
  withdrawal of the account used that nonce first. The bot then decides the account's target again,
  on the next nonce, once for that bar.

The bot takes the server's time for an order from a verified answer to a request it sent after the
bar completed, and counts on from there on its own monotonic clock.

## Bars and warm-up

The bot builds each market's reference price from the log's price updates, as the engine does, and
the price is exact once the log has passed the market's quote age limit. A market with trading
hours keeps only the quotes of one open session, and has no price while it is closed. Bars are
whole windows of the server's time, aligned to the Unix epoch, and are built only from moves of the
reference price, as the server's marks history records them. Bars from the history and bars built
from the live log therefore agree.

A strategy needs past bars before it can decide anything. Five seconds of log after the reference
price becomes exact, the bot warms each series up from the server's marks history, at the widest
resolution that divides the bar length:

- the bars before the bar in which the price became exact come from the history;
- that bar starts from the history's bucket for its window;
- the price moves that the bot has seen since then complete it.

## From target to order

Each target passes the account's controls before it becomes an order. The controls apply to a read
of the account that matches the log, and they set or refuse the order as follows:

- **Size.** The size of a new position comes from the sizing settings, held to the leverage cap.
- **Risk limits.** The number of open positions and their total notional value are capped. When
  the account's loss in a UTC day reaches the daily loss limit, the bot closes the account's
  positions and stops it until the next UTC day, and keeps it stopped across a restart.
- **Margin.** No order adds to a position unless the account's equity stays above the engine's
  margin requirement by the configured buffer, and its cash stays positive.
- **Minimum hold.** A strategy does not close a position until it has been held for the minimum
  time.
- **Trading hours.** The bot follows a market's hours as the engine prices it. A halt or a closure
  leaves no order, and the first bar after one is treated as a gap.

A share split in a traded market halts the bot, and so does a corporate action in a traded market
that fails. A warm-up starts after the last step in the history of 40% down or 67% up, which only a
split produces. The bot does not detect a split smaller than two for one, nor a split that happens
while it is stopped.

Orders are signed market orders toward each strategy's target, on the account's next nonce, quoted
at the last reference price. When the server declines an order, the bot logs the server's reason
and tries again on the next bar. When the engine refuses a reversal as a whole, the bot sends it
again as a reduce-only close followed by an opening order.

The bot does not send an order for a bar that is more than half a minute behind the server's time.
It measures that delay both by the time of the bar's stream message and by the server's time when
the order would be sent. Nor does the bot send an order before the log has passed the expiry of the
account's last order in that market, and of any order that an earlier process may have signed. An order's execution is
therefore always in the position before the next decision. Orders expire 30 seconds after they are
signed, and no order exceeds the engine's limit on a single instruction's size.

## Stopping

On SIGINT or SIGTERM, the bot first waits until the log has passed every order that could still
enter it, which takes at most 35 seconds. It then cancels each account's working orders. For an
account with `close_on_stop`, it also closes the account's positions:

- Each market is closed under its own strategy's maximum slippage, at the server's current
  reference price, which the bot reads again once its reading is two seconds old.
- The server closes at most a billion units in one request. While a close leaves part of a position
  open, the bot closes again on a fresh nonce, for at most a minute.

If the stop leaves a working order or a position behind, the bot raises an alert and exits with a
non-zero status. It does the same when it cannot confirm that it left none. The stop takes the wait
for the log, followed by a few requests for each account, paced under the rate limits. A second
signal does not cut it short. A supervisor's grace period must therefore be longer than the stop,
and longer still with many accounts. The [README](../README.md#stopping-the-bot) gives the figures.

## Halting

The bot halts in any of these cases:

- it reads an answer or a stream message signed by another key;
- it meets a log message or a market listing that it cannot follow;
- every account has stopped.

A halt changes nothing. The bot cancels no order and closes no position, and leaves the accounts as
they are for their owners to inspect. It raises a Critical alert and exits with a non-zero status.

## Funding commands

`trader accounts` acts for every account in the config, or for each one that `--account` names.

- `status` shows each account's eligibility declaration, wallet, balance, unclaimed MONKEY and
  withdrawals.
- `declare --class <professional|eligible-client>` shows the statement for that class of client,
  and records it, signed by the account, only for an account that you confirm in the terminal or
  with `--confirm <address>`. It needs only the trading server, and no `[chain]` section.
  Declarations are paced under the server's budget for new accounts, which
  `[node.limits] fresh_accounts` sets.
- `deposit --amount <USDC>` approves the vault for the amount and deposits it from the account's
  wallet, through the vault's `deposit`. With `--from-funding`, it deposits from the funding key's
  wallet, through `depositFor`. It then waits for the server to credit the deposit, and reports a
  deposit that the server holds until the account declares its eligibility. The bot sends a deposit
  again only once the vault holds none of the transactions signed for it, at a block by which
  another transaction has used the last one's nonce. A deposit therefore never goes in twice,
  whatever the endpoints' receipts say.
- `claim` claims each account's MONKEY with the server's proof, through the vault's
  `claimEmissions`. With a `[claims]` section, `trader run` claims by itself once the unclaimed
  amount reaches a threshold.
- `withdraw [--amount <USDC>]` requests a signed withdrawal. The withdrawal joins a first-in,
  first-out queue, and the bot pays each part that the queue approves with the vault's `withdraw`
  as its authorisation arrives, until nothing more of the request remains in the queue. The bot
  asks for a new authorisation when one has expired unpaid, and asks again after a while if none
  arrives. When the vault refuses an authorisation because of the chain's time or a change of
  signer, the bot waits for the authorisation to be replaced. Without an amount, the command pays
  what has already been approved.

Every payout goes to the account's own address. The vault pays a claim or a withdrawal to the
account it names, and the bot names only an account whose key it holds. The bot also checks that
each payout's authorisation names that account, and its token binding has no transfer function.
The optional funding key pays the gas for claims and withdrawals, and funds deposits through
`depositFor`, but never receives a payout.

A payout or a claim that another sender made first counts as done. After a stale nonce, the bot
sends the request again on the current nonce. After an answer that asks it to retry later, it sends
the request again within the action's deadline. A withdrawal is sent again on the same nonce, which
the server accepts at most once, so a stale nonce then is reported rather than followed. When an
answer never arrives, the bot checks the server's record before it asks again.

A command fails when the action of any account failed, including a refused declaration. The bot
unlocks the funding key only for the actions that the funding key pays for.

## The custody chain

The bot reaches the chain through one or more JSON-RPC endpoints, connecting to each on first use.

- A read goes only to an endpoint that has confirmed that it is on the vault's chain. The bot moves
  past an endpoint that fails or returns an error other than a revert. After one of the bot's own
  transactions lands, it also moves past an endpoint that is behind that transaction's block.
- Every transaction is estimated first, and is not sent if it would revert. The bot sends it to
  every endpoint, and follows it to its receipt even when no endpoint answered.
- The bot estimates and signs a transaction again only once another transaction from the same key
  has used its nonce, and every endpoint, poll after poll, holds no receipt for it.
- A transaction that reverts is reported with the contract's reason.
- No transaction offers a fee above the configured cap, so an endpoint that misquotes the price
  cannot spend the payer's ether.
- An endpoint's URL, and each part of it that may carry an API key, is replaced in every log line,
  output line and error, whatever the log level.

## Keys

The bot reads each account key from an encrypted Web3 Secret Storage keystore, the file that wallets
export. A development build also reads a key from a file of hexadecimal characters. The bot holds a
key only as a signing key that erases its secret when it is dropped, and never prints it. A
production build refuses the keys whose private keys this repository publishes for development.

The `production` Cargo feature, which release binaries and images are built with, refuses at
startup a config that only suits a development server: a simulated attestation, an extra
attestation root, a pinned TLS key, a hex key file, or a plain-HTTP chain endpoint.

## Metrics

With a `[metrics]` section, the bot serves Prometheus metrics on a loopback address: the orders it
placed and the orders declined, and the executions, for each account and market; each account's
equity and margin level; and the stream's lag. If the metrics listener fails, the bot stops as it
would on a signal. [Configuration](configuration.md#metrics) lists each metric.

## Performance

After warm-up, applying one log message allocates memory at most twice, for any number of
accounts, and both allocations are made while the JSON parser reads the message's nesting.
`crates/trader/tests/alloc.rs` holds every message of a synthetic log to that bound.
`crates/trader/tests/hot_path.rs` replays an hour of a synthetic log through 1,000 accounts, and
holds the time per message and the memory per account to budgets.
