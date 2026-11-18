# Security policy

trader holds the keys of funded accounts. It signs their orders and their transactions on the
custody chain, so a vulnerability in it can cost its operator money. We ask you to report a
vulnerability privately, and to give us time to fix it before it becomes public. This policy
explains how to reach us, what you can expect from us in return, what it covers, and what each part
of the bot is trusted for.

The bot trades on a monkeytrading trading server, which its configuration and commands call the
node.

## Reporting a vulnerability

Email your report to william.jevons@proton.me. Mail sent from a Proton account is end-to-end
encrypted. For senders on other providers, we will publish a PGP key in this file.

Do not describe a vulnerability in a public issue, pull request or discussion, even in outline. Keep
the details out of an email's subject line.

A report is easiest to act on when it includes:

- the affected component, and the commit of `main` you tested;
- what an attacker can achieve, and what access they need first;
- how to reproduce it. A failing test, or a script against a local bot and a local trading server,
  makes the strongest case;
- whether you have a fix in mind, and whether anyone else knows of the issue;
- how you would like to be credited.

## What happens next

We aim to:

- acknowledge your report within three business days;
- tell you, within ten business days of receiving it, whether we can reproduce the issue, how severe
  we judge it, and what we plan to do;
- fix the vulnerability within 90 days of your report, or within 30 days if it is one of the
  [findings that matter most](#findings-that-matter-most);
- keep you informed until the issue is resolved.

If we conclude that a report does not describe a vulnerability, we explain why.

### Coordinated disclosure

We ask you to keep the details of a vulnerability private until a fix has shipped, or until 90 days
after you reported it, whichever comes first. A fix has shipped when it is on `main`. If a fix needs
longer, we will tell you why before the deadline and agree a new date with you.

Once the fix has shipped, we publish an advisory in this repository. It describes the vulnerability,
its impact and the fix, and it credits you by name unless you would rather it did not. You are free
to publish your own account from the day the fix ships. If we conclude that a report does not
describe a vulnerability, you may disclose it once we have told you so.

## Safe harbour

We will not take or support legal action against anyone who researches and reports a vulnerability
in good faith under this policy. If a third party does, we will make it known that the research was
authorised.

Good faith means:

- demonstrating a vulnerability on a bot, trading server and chain that you run yourself, rather
  than on someone else's deployment;
- never accessing anyone else's keys or data, and never changing or destroying them;
- not degrading any service for other users;
- keeping the details private, as this policy asks.

If you are unsure whether something you plan to do is covered, ask us before you do it.

## Scope

The policy covers the code in this repository and everything built from it:

| Component | What it is |
| --- | --- |
| `trader-core` | The strategy library: indicators, strategies, sizing and risk limits, with no IO. |
| `trader` | The binary. It verifies the trading server's attestation, reads its API and public log, signs orders and on-chain transactions with the accounts' keys, and serves the console. |
| The console | The web app in `console/`, which the binary embeds and serves over HTTPS behind a login. |
| The image and the releases | The image that the `Dockerfile` builds, and the binaries, checksums and image that the release workflow publishes. |
| CI workflows and scripts | They guard what merges and what is released. |

The engine, its trading server and its contracts have their own
[security policy](https://github.com/monkey-trading/engine/blob/main/SECURITY.md). Please report a
weakness in them there. A weakness in a development setting, such as a mock trading server or the
dev stack's published test keys, is in scope only when a production configuration can reach it.

trader has not been released, and nothing here has held real funds.

### Findings that matter most

The findings that matter most let someone:

- obtain or use a key the bot holds;
- make the bot sign an order that its operator did not configure;
- make the bot act on an answer from the trading server that fails its signature check;
- make a production build trust a signer that no attestation, verified against the operator's
  release manifests, names;
- move funds away from an account's own key through the console;
- read a key or the login password through the console;
- act through the console from another site's page;
- reach the console without the operator's login.

### What counts as a new finding

Each section below states what one part of the bot guarantees, and then its known limits. A report
that rests only on a limit stated here, or in an open issue, is not a new finding, although its
details remain private under the disclosure timeline above. A way around a check that exists is a
new finding.

## What each part is trusted for, and its known limits

### The connection to the trading server

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

### The custody chain and funding

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

### The console

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

### Keys and the host

The bot reads each key from an encrypted keystore, unlocks it once at startup and holds it only as a
signing key, which zeroes its secret when it is dropped. The bot never prints a key. A production
build refuses a plain hex key, and the keys this repository publishes for development. Prometheus
metrics, which name the accounts and their equity, are served only on a loopback address.

Known limits:

- The bot trusts the machine it runs on. Anyone who can read the keystores and their passwords,
  whether the configuration takes a password from a file or from the bot's environment, holds the
  keys. Anyone who can read the bot's memory holds the unlocked keys.

### Releases

Only the maintainer can create a version tag, which the repository's tag ruleset enforces. The
release workflow publishes only a tag that matches the crate's version on a commit of `main`. The
image and the binaries are built with every input pinned.

Known limits:

- A release publishes the binaries' SHA-256 checksums in the same GitHub release as the binaries,
  with no separate signature. The checksums detect a damaged download, but not a release that
  someone able to publish to the repository has replaced.

## Supported versions

trader has not been released. Only the latest commit on `main` is supported, so please check that an
issue still reproduces there before you report it.

## Bug bounty

There is no bug bounty.
