# The console

With a `[console]` section in its config, the bot serves a web console: the figures of every
account it trades, and controls to start, stop and close them, in a browser, over HTTPS, behind a
login. With a `[chain]` section as well, the console also funds the accounts and withdraws from
them. This page describes what the console shows, what its controls do, how it is secured, and
the API that the app uses.

## Signing in

The console listens on every interface at port 8443 by default, so it is reachable at
`https://<host>:8443`. `[console] bind` moves it.

On its first run, the bot generates a self-signed certificate and a login password. It keeps the
certificate and its key in the console's data directory, prints the password once, and keeps only
an Argon2id hash of the password, in `login.argon2`. Every start prints the certificate's SHA-256
fingerprint. Open the console, check that the certificate your browser shows has that fingerprint,
and sign in with the password. To get a new password, delete `login.argon2` and restart the bot.
`[console] tls` serves a certificate and key of your own in place of the generated pair.

**The console's controls can stop and close every account the bot trades. Keep the password
secret, and compare the certificate's fingerprint before you sign in, because a server that
presents another certificate can capture the password.**

## What it shows

The overview lists every account in the config, in the config's order, with its equity, profit and
loss, margin level, positions and MONKEY, and the totals across all accounts. Each account's page
shows:

- its balance, equity, margin requirement and margin level;
- its positions, each with its market's reference price, its unrealised profit or loss as the
  engine values it, and the price at which the account would be closed out;
- its working orders and its latest executions;
- its unrealised profit, which is equity less balance, and its realised profit over the account's
  whole history of executions, after profit fees;
- its MONKEY entitlement;
- its status, which is `running`, `stopped`, `flattening` or `halted`;
- each strategy's kind, parameters, the range within which each parameter may be changed, its
  warm-up and its current target;
- an equity curve of the last 24 hours, one point a minute.

While the daily loss limit holds an account stopped, its status says until when, and the day's
loss. The equity curve is kept in memory, so a restart starts it again.

An account that could not start is listed as halted, with the reason and no figures or controls.
When its key would not unlock, the console names the key file in place of an address. The totals
count it among the accounts, but not among those reporting figures.

Every figure comes from the trading server's verified answers. The console reads each account every
`[console] refresh_interval`, five seconds by default, through the same paced connections that the
bot trades over. Its reads use at most half of the server's per-address rate limit, so that orders
always have the rest. With many accounts, a round of reads therefore takes longer. An answer signed
by another key stops the bot, as it does for the bot's own reads. Figures update in the browser
over a WebSocket feed, which sends what has changed once a second. A control that fails says so
next to the control.

## Controls

- **Start and stop** an account, or one of its strategies.
- **Change a strategy's parameters** within the bounds that the config sets. The bot builds the
  strategy afresh and warms its series up again from the server's marks history, so the strategy
  decides nothing until the warm-up is complete.
- **Flatten** an account. The bot stops the account, waits for the log to pass its last order,
  cancels its working orders and closes every position. The account cannot be started again until
  the flatten has finished.

The bot acts on each request, and saves the controls in the data directory's `accounts/`
directory, so a restart keeps an account stopped. If a stop cannot be saved, the stop still holds,
and the console says that it was not saved.

## Funding

With a `[chain]` section, the Funding page shows each account's eligibility declaration, wallet,
balance, unclaimed MONKEY and withdrawals. It declares, deposits, claims and withdraws for the
accounts you select, and shows each action's progress until its transactions land. The bot then
unlocks the `[funding]` key when it starts, and exits if the key does not unlock.

- Each action runs as a background job for each account named in the request, and the job ends
  once its transactions land. An account whose job has not ended cannot start another.
- A declaration is refused unless it carries the statement of the declared class of client.
- A request never names a destination, so every payout goes to the account's own address. No route
  moves funds away from an account's own key, and no route returns a key.
- The accounts listed are every account whose key unlocked, including those that could not start
  because the server had not yet credited them. A bot none of whose accounts started still serves
  them.
- A job shows each transaction it sent and its state. Jobs are kept in memory, so a restart ends
  them, and logs each one that had not ended. The figures then show what landed.
- Four jobs can work at once. A job holds its place only while it signs, sends or follows a
  transaction, or asks the server something, so jobs that wait for a credit or a payout do not hold
  up the short ones.
- One key sends one transaction at a time, and one deposit from the funding key at a time sets and
  spends its allowance.
- A transaction that is still pending when its job ends is followed to its receipt for an hour, and
  its row shows the outcome.

While the console is open, the bot reads the funding figures in the background, using at most a
quarter of the server's per-address rate limit. It reads first the accounts that it has never read
and those that a job has changed, and answers from its latest reads.

## Security

- Sessions are random tokens in a `__Host-session` cookie that is `HttpOnly`, `Secure` and
  `SameSite=Strict`. The bot keeps only a hash of each token. A session ends when you sign out, or
  after twelve hours.
- Each client may try five passwords at once, and gains one more attempt every twelve seconds. The
  bot checks one password at a time.
- Every route except the login needs a session. Every request that changes something, and the live
  feed, must come from the console's own origin.
- A request's body must arrive within ten seconds, and may be at most 4 KiB. A funding request may
  be larger, so that it can name every account the bot trades.
- Every answer carries a strict Content Security Policy and the other standard security headers.

## The API

The app uses a JSON API under `/api`, and every route but `POST /api/login` needs a session.

| Route | Purpose |
| --- | --- |
| `POST /api/login`, `POST /api/logout`, `GET /api/session` | Sign in, sign out, and check whether the session is live |
| `GET /api/accounts` | Every account with its newest figures, and the totals |
| `GET /api/accounts/{address}` | One account, with its whole equity curve |
| `GET /api/accounts/{address}/fills` | The account's latest executions |
| `POST /api/accounts/{address}/start`, `stop`, `flatten` | The account's controls |
| `POST /api/accounts/{address}/strategies/{id}/start`, `stop` | A strategy's controls |
| `PUT /api/accounts/{address}/strategies/{id}/params` | A strategy's parameters, within its bounds |
| `GET /api/live` | The WebSocket feed of what changed, once a second |
| `GET /api/funding` | Each account's funding figures and jobs |
| `POST /api/funding/declare`, `deposit`, `claim`, `withdraw` | The funding actions, for the accounts the request names |

Whole numbers are JSON numbers, and decimal amounts are strings. The list of accounts and the live
feed carry only the newest point of each equity curve, which a client appends to the curve it read.
A refusal's body is `{"error": "<message>", "code": "<code>"}`. A client matches on the code, such
as `session_required`, `foreign_origin`, `rate_limited`, `body_timeout`, `flatten_in_progress`,
`account_halted` or `out_of_bounds`, and shows the message. An account that could not start
carries a code that says why: `key_locked`, `account_refused`, `account_unread`, `no_balance`,
`position_elsewhere`, `controls_unreadable` or `market_unsupported`.

## The app

The app is a Vite, React and TypeScript single-page app in `console/`. `npm run build` writes it to
`console/dist`, and the bot embeds that directory when the bot is built after it. The bot serves the
app at `/`, and routes any other page path to its `index.html`. A binary built without the app
serves a page that says so. Building the app needs the Node.js version in `console/.nvmrc`, and npm.

```shell
cd console
npm ci
npm run build
```

`npm run build` writes `console/dist`. Build the bot after it, so that the bot embeds the app.

To work on the app without a bot, run the stand-in API, which serves three accounts over plain
HTTP on loopback and prints its address and password:

```shell
npm run stub
```

`npm run dev` serves the app with Vite's development server on `127.0.0.1`, and forwards `/api` to
the bot's console at `https://127.0.0.1:8443`, or to the address in `CONSOLE_BOT_URL`, such as the
stand-in's.
