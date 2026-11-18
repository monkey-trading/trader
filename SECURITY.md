# Security policy

trader holds the keys of funded accounts. It signs their orders and their transactions on the
custody chain, so a vulnerability in it can cost its operator money. We ask you to report a
vulnerability privately, and to give us time to fix it before it becomes public. This policy
explains how to reach us, what you can expect from us in return, and what it covers.

The [security model](docs/security-model.md) states what each part of the bot guarantees, and its
known limits.

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

The [security model](docs/security-model.md) states what each of these parts guarantees, and its
known limits. trader has not been released, and nothing here has held real funds.

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

Each section of the [security model](docs/security-model.md) states what one part of the bot
guarantees, and then its known limits. A report that rests only on a limit stated there, or in an
open issue, is not a new finding, although its details remain private under the disclosure timeline
above. A way around a check that exists is a new finding.

## Supported versions

trader has not been released. Only the latest commit on `main` is supported, so please check that an
issue still reproduces there before you report it.

## Bug bounty

There is no bug bounty.
