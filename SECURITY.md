# Security policy

trader is built to hold keys and sign orders for funded accounts, so a vulnerability in it can cost
its operator money. We ask that you report one privately and give us time to fix it before it becomes
public. This policy explains how to reach us, what you can expect in return, and what it covers.

## Reporting a vulnerability

Email reports to william.jevons@proton.me. Please do not describe a vulnerability in a public
issue, pull request, or discussion, even in outline. Mail sent from a Proton account is end-to-end
encrypted; a PGP key for senders on other providers will be published in this file. Either way,
please keep the details out of the subject line.

A report is easiest to act on when it names the affected component and the commit of `main` you
tested, says what an attacker can achieve and what access they need first, and shows how to
reproduce it. A failing test or a script against a local bot and a local node makes the strongest
case. Tell us, too, whether you have a fix in mind, whether anyone else knows of the issue, and how
you would like to be credited.

## What happens next

We aim to acknowledge a report within three business days, and within ten business days of
receiving it to tell you whether we can reproduce the issue, how severe we judge it, and what we
plan to do. We aim to fix a vulnerability within 90 days of your report, or within 30 days if it is
of a kind that the Scope section below lists as mattering most, and we will keep you informed until
it is resolved. If we conclude that a report does not describe a vulnerability, we will explain
why.

## Coordinated disclosure

We ask that you keep the details of a vulnerability private until a fix has shipped, or until 90
days after you reported it, whichever comes first. A fix has shipped when it is on `main`. If a fix
needs longer, we will tell you why before the deadline and agree a new date with you. Once the fix
has shipped, we publish an advisory in this repository that describes the vulnerability, its
impact, and the fix, and credits you by name unless you would rather it did not. You are free to
publish your own account from the day the fix ships. If we conclude that a report does not describe
a vulnerability, you may disclose it once we have told you so.

## Safe harbour

We will not take or support legal action against anyone who researches and reports a vulnerability
in good faith under this policy, and if a third party does, we will make it known that the research
was authorised. Good faith means demonstrating a vulnerability on a bot, node, and chain you run
yourself rather than on someone else's deployment; never accessing anyone else's keys or data, and
never changing or destroying them; not degrading any service for other users; and keeping the
details private as this policy asks. If you are unsure whether something you plan to do is covered,
ask us before you do it.

## Scope

The policy covers the code in this repository and what is built from it: `trader-core`, the
strategy logic; `trader`, the binary, which talks to a monkeytrading node, signs for its keys, and
serves the console and its API over HTTPS behind a login; the console in `console/`, a web app the
binary embeds; and the image the `Dockerfile` builds and the release workflow publishes. The CI
workflows and scripts in the repository are in scope too, as they guard what merges and what is
released.

The findings that matter most let someone obtain or use a key the bot holds, make the bot sign an
order its operator did not configure, make the bot act on an answer from the node that fails its
signature check, make a production build trust a signer that no attestation verified against the
operator's release manifests names, move funds away from an account's own key through the console,
read a key or the login password through it, act through the console from another site's page, or
reach the console without the operator's login.

The engine, its node, and its contracts have their own policy in the
[engine repository](https://github.com/monkey-trading/engine/blob/main/SECURITY.md); report a
weakness in them there. A weakness in a development setting, such as a mock node or the dev stack's
published test keys, is in scope only when a production configuration can reach it.

## Supported versions

trader has not been released. Until it is, only the latest commit on `main` is supported, so
please check that an issue still reproduces there before reporting it.
