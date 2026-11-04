// A stand-in for the bot's accounts: integer money in units of 10^-9, advanced by a seeded generator so a
// run is repeatable. It speaks the console API's JSON; it holds no key and trades nothing.

const SCALE = 1_000_000_000n;

/** Renders units as the shortest exact decimal, as the engine's Fixed does. */
export function text(units) {
  const sign = units < 0n ? "-" : "";
  const abs = units < 0n ? -units : units;
  const whole = abs / SCALE;
  const fraction = (abs % SCALE).toString().padStart(9, "0").replace(/0+$/, "");
  return fraction === "" ? `${sign}${whole}` : `${sign}${whole}.${fraction}`;
}

/** Parses a plain decimal into units, or null. */
export function units(value) {
  const match = /^(-?)(\d{1,15})(?:\.(\d{1,9}))?$/.exec(String(value));
  if (match === null) {
    return null;
  }
  const magnitude = BigInt(match[2]) * SCALE + BigInt((match[3] ?? "").padEnd(9, "0"));
  return match[1] === "-" ? -magnitude : magnitude;
}

const PARAMS = {
  sma_crossover: [
    ["fast", "integer", "5", "2", "50"],
    ["slow", "integer", "20", "5", "200"],
  ],
  bollinger_reversion: [
    ["period", "integer", "20", "5", "100"],
    ["width", "decimal", "2", "0.5", "4"],
  ],
};

function strategy(id, kind, market, state) {
  return {
    id,
    kind,
    market,
    bar: "1m",
    state,
    reason: null,
    params: PARAMS[kind].map(([name, type, value, min, max]) => ({ name, kind: type, value, min, max })),
  };
}

function account(index, name, balance, positions) {
  return {
    address: `0x${String(index).repeat(40)}`,
    name,
    state: "running",
    stateReason: null,
    balance: units(balance),
    realised: 0n,
    positions: positions.map(([market, qty, entry]) => ({ market, qty: units(qty), entry: units(entry) })),
    orders: [],
    fills: [],
    curve: [],
    strategies: [
      strategy(`${name}-sma`, "sma_crossover", "BTC-USD", "running"),
      strategy(`${name}-bollinger`, "bollinger_reversion", "ETH-USD", "stopped"),
    ],
    earned: units("12.5") * BigInt(index),
    claimed: units("5") * BigInt(index),
  };
}

/** Three accounts, one with an open short and one flat. */
export function createModel(now) {
  const accounts = [
    account(1, "alpha", "10000", [["BTC-USD", "0.1", "60000"]]),
    account(2, "beta", "2500.55", [["ETH-USD", "-2", "3000"]]),
    account(3, "gamma", "800", []),
  ];
  // The third account's strategies cannot start: a refusal the console must show at the control.
  accounts[2].blocked = "the account is halted by its daily loss limit";
  return { accounts, marks: { "BTC-USD": units("61000"), "ETH-USD": units("2950") }, seq: 1, seed: 7n, now };
}

function unrealised(model, position) {
  return ((model.marks[position.market] - position.entry) * position.qty) / SCALE;
}

function marginLevel(model, item) {
  const notional = item.positions.reduce((sum, p) => sum + (abs(p.qty) * model.marks[p.market]) / SCALE, 0n);
  const requirement = notional / 20n;
  return requirement === 0n ? null : text((equity(model, item) * SCALE) / requirement);
}

function abs(value) {
  return value < 0n ? -value : value;
}

function equity(model, item) {
  return item.balance + item.positions.reduce((sum, p) => sum + unrealised(model, p), 0n);
}

/** The account's figures as the console's list reads them. */
export function summary(model, item) {
  return {
    address: item.address,
    name: item.name,
    state: item.state,
    stateReason: item.stateReason,
    balance: text(item.balance),
    equity: text(equity(model, item)),
    realisedPnl: text(item.realised),
    unrealisedPnl: text(item.positions.reduce((sum, p) => sum + unrealised(model, p), 0n)),
    marginLevel: marginLevel(model, item),
    openPositions: item.positions.length,
    monkeyEarned: text(item.earned),
    monkeyClaimed: text(item.claimed),
  };
}

/** The part of an account the feed carries as an `account` message. */
export function live(model, item) {
  return {
    ...summary(model, item),
    positions: item.positions.map((p) => ({
      market: p.market,
      qty: text(p.qty),
      entry: text(p.entry),
      mark: text(model.marks[p.market]),
      unrealisedPnl: text(unrealised(model, p)),
      closeout: text((p.entry * 8n) / 10n),
    })),
    orders: item.orders,
    strategies: item.strategies,
  };
}

/** The account's whole page. */
export function detail(model, item) {
  return { ...live(model, item), fills: item.fills.map((f) => f), curve: item.curve.map((c) => c) };
}

function next(model) {
  model.seed = (model.seed * 6364136223846793005n + 1442695040888963407n) % (1n << 63n);
  return Number((model.seed >> 20n) % 2001n) - 1000;
}

/** Moves both marks a little, records a curve point per account, and returns the feed messages. */
export function tick(model) {
  model.seq += 1;
  for (const market of Object.keys(model.marks)) {
    model.marks[market] += (model.marks[market] * BigInt(next(model))) / 100000n;
  }
  const messages = [];
  for (const item of model.accounts) {
    const point = { time: model.now(), equity: text(equity(model, item)) };
    item.curve = [...item.curve, point].slice(-500);
    messages.push({ type: "account", seq: model.seq, account: live(model, item) });
    messages.push({ type: "curve", seq: model.seq, address: item.address, point });
  }
  return messages;
}

/** Closes the account's positions at the mark, as a flatten does, and records the fills. */
export function flatten(model, item) {
  for (const p of item.positions) {
    item.realised += unrealised(model, p);
    item.balance += unrealised(model, p);
    item.fills = [
      {
        id: `f${model.seq}-${p.market}`,
        time: model.now(),
        market: p.market,
        side: p.qty > 0n ? "sell" : "buy",
        qty: text(abs(p.qty)),
        price: text(model.marks[p.market]),
        fee: "0.5",
        realisedPnl: text(unrealised(model, p)),
      },
      ...item.fills,
    ];
  }
  item.positions = [];
  model.seq += 1;
}
