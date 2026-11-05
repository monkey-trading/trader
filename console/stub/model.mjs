// A stand-in for the bot's accounts, speaking the console API's JSON as the bot serves it: integer money in
// units of 10^-9 (the engine's Fixed), MONKEY in base units, advanced by a seeded generator so a run is
// repeatable. It holds no key and trades nothing.

const SCALE = 1_000_000_000n;
const MONKEY_BASE = 10n ** 18n;

/** Renders units as the shortest exact decimal, as the engine's Fixed does. */
export function text(units) {
  const sign = units < 0n ? "-" : "";
  const abs = units < 0n ? -units : units;
  const whole = abs / SCALE;
  const fraction = (abs % SCALE).toString().padStart(9, "0").replace(/0+$/, "");
  return fraction === "" ? `${sign}${whole}` : `${sign}${whole}.${fraction}`;
}

function parse(value) {
  const [whole, fraction = ""] = value.split(".");
  return BigInt(whole) * SCALE + BigInt(fraction.padEnd(9, "0"));
}

function abs(value) {
  return value < 0n ? -value : value;
}

const SYMBOLS = { 1: "BTC-USD", 2: "ETH-USD" };

function account(index, balance, positions, orders) {
  return {
    address: `0x${String(index).repeat(40)}`,
    state: "running",
    halt: null,
    balance: parse(balance),
    realised: 0n,
    positions: positions.map(([market, qty, entry]) => ({ market, qty: parse(qty), entry: parse(entry) })),
    orders,
    fills: [],
    curve: [],
    strategy: {
      id: 0,
      kind: "sma_crossover",
      market: "BTC-USD",
      bar: "1m",
      running: true,
      params: { fast: 5, slow: 20 },
      bounds: { fast: [2, 50], slow: [5, 200] },
      warmed_up: true,
      target: "long",
    },
    earned: BigInt(index) * 12n * MONKEY_BASE + MONKEY_BASE / 2n,
    claimable: BigInt(index) * 10n * MONKEY_BASE,
    version: 1,
    fillsVersion: 1,
  };
}

/** Three accounts: one long, one short with a resting order, one halted-style account whose strategy will not start. */
export function createModel(now) {
  const accounts = [
    account(1, "10000", [[1, "0.1", "60000"]], []),
    account(2, "2500.55", [[2, "-2", "3000"]], [
      { order_id: 9, client_id: 1, market: 2, side: "buy", qty: "1", kind: { kind: "limit", price: "2800" }, reduce_only: false },
    ]),
    account(3, "800", [], []),
  ];
  // The third account's periods are not the console's to change, and its strategy refuses to start.
  accounts[2].strategy.running = false;
  accounts[2].strategy.bounds = null;
  accounts[2].blocked = "the account is over its daily loss limit";
  return { accounts, marks: { 1: parse("61000"), 2: parse("2950") }, seed: 7n, now };
}

function unrealised(model, position) {
  return ((model.marks[position.market] - position.entry) * position.qty) / SCALE;
}

function equity(model, item) {
  return item.balance + item.positions.reduce((sum, p) => sum + unrealised(model, p), 0n);
}

function margin(model, item) {
  return item.positions.reduce((sum, p) => sum + (abs(p.qty) * model.marks[p.market]) / SCALE, 0n) / 20n;
}

function level(equityUnits, marginUnits) {
  return marginUnits === 0n ? null : Number((equityUnits * 10_000n) / marginUnits);
}

function figures(model, item) {
  const eq = equity(model, item);
  const req = margin(model, item);
  return {
    balance: text(item.balance),
    equity: text(eq),
    margin: text(req),
    margin_level_bps: level(eq, req),
    unrealised_pnl: text(eq - item.balance),
    realised_pnl: text(item.realised),
    positions: item.positions.map((p) => ({
      market: p.market,
      symbol: SYMBOLS[p.market],
      qty: text(p.qty),
      entry: text(p.entry),
      mark: text(model.marks[p.market]),
      unrealised_pnl: text(unrealised(model, p)),
      closeout: text((p.entry * 8n) / 10n),
    })),
    orders: item.orders,
    monkey: { entitled: String(item.claimable), newest_entitled: String(item.earned), claimed: null },
    time_ms: model.now(),
  };
}

/** One account as the API shows it. */
export function view(model, item) {
  return {
    address: item.address,
    state: item.state,
    halt: item.halt,
    strategies: [item.strategy],
    figures: figures(model, item),
    error: null,
    curve: item.curve,
  };
}

/** The accounts and their totals, as `GET /api/accounts` and the feed's snapshot carry them. */
export function overview(model) {
  const eq = model.accounts.reduce((sum, item) => sum + equity(model, item), 0n);
  const req = model.accounts.reduce((sum, item) => sum + margin(model, item), 0n);
  const sum = (pick) => model.accounts.reduce((total, item) => total + pick(item), 0n);
  return {
    accounts: model.accounts.map((item) => view(model, item)),
    totals: {
      accounts: model.accounts.length,
      reporting: model.accounts.length,
      balance: text(sum((item) => item.balance)),
      equity: text(eq),
      margin: text(req),
      margin_level_bps: level(eq, req),
      unrealised_pnl: text(eq - sum((item) => item.balance)),
      realised_pnl: text(sum((item) => item.realised)),
      monkey: {
        entitled: String(sum((item) => item.claimable)),
        newest_entitled: String(sum((item) => item.earned)),
        claimed: null,
      },
    },
  };
}

/** The account's fills as `GET /api/accounts/{address}/fills` carries them. */
export function fills(item) {
  return { fills: item.fills };
}

function next(model) {
  model.seed = (model.seed * 6364136223846793005n + 1442695040888963407n) % (1n << 63n);
  return Number((model.seed >> 20n) % 2001n) - 1000;
}

/** Moves both marks a little and records a curve point per account; returns the feed messages. */
export function tick(model) {
  for (const market of Object.keys(model.marks)) {
    model.marks[market] += (model.marks[market] * BigInt(next(model))) / 100000n;
  }
  const messages = [];
  for (const item of model.accounts) {
    item.curve = [...item.curve, { time_ms: model.now(), equity: text(equity(model, item)) }].slice(-1440);
    messages.push({ type: "account", account: view(model, item) });
  }
  messages.push({ type: "totals", totals: overview(model).totals });
  return messages;
}

/** Closes the account's positions at the mark, as a flatten does, and records the fills. */
export function flatten(model, item) {
  item.state = "stopped";
  item.strategy.running = false;
  for (const p of item.positions) {
    const pnl = unrealised(model, p);
    item.realised += pnl;
    item.balance += pnl;
    item.fills = [
      {
        cursor: `${model.now()}:${p.market}`,
        ts: model.now(),
        account: item.address,
        order_id: 100 + p.market,
        market: p.market,
        side: p.qty > 0n ? "sell" : "buy",
        qty: text(abs(p.qty)),
        price: text(model.marks[p.market]),
        index: text(model.marks[p.market]),
        spread_bps: 0,
        realised_pnl: text(pnl),
        profit_fee: "0.5",
        position: "0",
        liquidation: false,
        symbol: SYMBOLS[p.market],
      },
      ...item.fills,
    ];
  }
  item.positions = [];
  item.orders = [];
  item.fillsVersion += 1;
}
