# Security model

This page states what each part of trader guarantees, and then its known limits. The
[security policy](../SECURITY.md) explains how to report a vulnerability, what is in scope, and when
a report counts as a new finding. A report that rests only on a limit stated here is not a new
finding.

- [The connection to the trading server](#the-connection-to-the-trading-server)
- [The custody chain and funding](#the-custody-chain-and-funding)
- [The console](#the-console)
- [Keys and the host](#keys-and-the-host)
- [Releases](#releases)

The bot trades on a monkeytrading trading server, which the bot's configuration and commands call
the node.

## The connection to the trading server

Before any other call, the bot verifies the trading server's attestation against the release
manifests its configuration names. The quote's certificate chain must reach a vendor root pinned in
`engine-verify`, and the quote must match one of the releases. From then on, the bot accepts only
connections that present the TLS key the attestation names. Every call carries a fresh challenge,
and the bot uses an answer only once the attested enclave key's signature over the answer, the
request and the challenge verifies. The bot checks every frame of its stream connection against the
attested signer before it reads the frame. A production build refuses a mock attestation, an extra
attestation root and a pinned TLS key.

When an answer fails its signature check, the bot halts, exits with a non-zero status and leaves the
accounts as they are. A stream connection that fails a check, or falls silent for longer than the
stream timeout, is closed and replaced.

Known limits:

- The bot accepts any release that the configured manifests list. Whoever writes the configuration
  therefore decides which code the bot trusts.
- The bot checks the attestation once, before its first call, and not again while it runs. A
  certificate that the vendor revokes after that is noticed at the bot's next start.
- Anyone between the bot and the trading server can delay or drop traffic, and signatures cannot
  prevent it.
- A supervisor that restarts the bot after it halts resumes trading without anyone examining the
  accounts first.
- If the bot is killed while it stops, the accounts it had not yet reached keep their working orders
  and positions, and nothing reports which accounts those are.

## The custody chain and funding

Every payout goes to the address of the account that signs it. The vault pays a claim or a
withdrawal to the account it names, and the bot names only accounts whose keys it holds. The
optional funding key pays gas and funds deposits through the vault's `depositFor`, and it never
receives a payout. The bot estimates each transaction first and does not send one that would revert,
and no transaction offers more than `max_fee_gwei` per unit of gas. A production build takes only
`https://` chain endpoints. Logs and errors name an endpoint by its place in the configuration,
because its URL may carry credentials.

Known limits:

- The bot believes what its chain endpoints report about the vault and the tokens, and checks no
  proof. An endpoint that lies can make the console show wrong balances, claims and withdrawals, or
  make the bot send a transaction that fails and costs its gas. It cannot redirect a payout.

## The console

The console serves HTTPS only, behind one login password. The bot generates the password on its
first run, prints it once, and keeps only its Argon2id hash. The password is 26 random characters,
or 130 bits. Each client, an IPv4 address or an IPv6 /64, may try to log in five times at once, and
then once every 12 seconds. A session is a random token in an `HttpOnly`, `Secure`,
`SameSite=Strict` cookie, which the bot holds only as its SHA-256 hash, and which lasts 12 hours.
Every request that changes state, or opens the live feed, must come from the console's own origin.
No route returns a key, and nothing the console can do moves funds away from an account's own key.

Known limits:

- Unless its configuration names a certificate, the console uses a self-signed certificate, which a
  browser cannot check. The bot prints the certificate's SHA-256 fingerprint at every start. A user
  who does not compare it with the fingerprint the browser shows can give the password to another
  server.
- The console listens on every interface, on port 8443, unless `bind` sets another address. Anyone
  who can reach that port can try to log in.
- Whoever holds the password controls every account the bot trades. They can start and stop accounts
  and strategies, close every position, change strategy parameters within the bot's bounds, and
  declare, deposit, claim and withdraw. Claims and withdrawals still pay only each account's own
  address.

## Keys and the host

The bot reads each key from an encrypted keystore, unlocks it once at startup and holds it only as a
signing key, which zeroes its secret when it is dropped. The bot never prints a key. A production
build refuses a plain hex key, and the keys this repository publishes for development. Prometheus
metrics, which name the accounts and their equity, are served only on a loopback address.

Known limits:

- The bot trusts the machine it runs on. Anyone who can read the keystores and their passwords,
  whether the configuration takes a password from a file or from the bot's environment, holds the
  keys. Anyone who can read the bot's memory holds the unlocked keys.

## Releases

Only the maintainer can create a version tag, which the repository's tag ruleset enforces. The
release workflow publishes only a tag that matches the crate's version on a commit of `main`. The
image and the binaries are built with every input pinned.

Known limits:

- A release publishes the binaries' SHA-256 checksums in the same GitHub release as the binaries,
  with no separate signature. The checksums detect a damaged download, but not a release that
  someone able to publish to the repository has replaced.
