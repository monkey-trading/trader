/** Bodies shaped as the bot's console API serves them, for the tests of the parsers, stores and pages. */

/** The address the fixtures' account has. */
export const ADDRESS = `0x${"11".repeat(20)}`;

/** An account's figures. */
export function figuresBody(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    balance: "10000",
    equity: "10100.5",
    margin: "50",
    margin_level_bps: 202_100,
    unrealised_pnl: "100.5",
    realised_pnl: "-3.25",
    positions: [
      { market: 1, symbol: "BTC-USD", qty: "0.1", entry: "60000", mark: "61000", unrealised_pnl: "100", closeout: "48000" },
    ],
    orders: [{ order_id: 9, client_id: 1, market: 2, side: "buy", qty: "1", kind: { kind: "limit", price: "50" }, reduce_only: false }],
    monkey: { entitled: "5000000000000000000", newest_entitled: "12500000000000000000", claimed: null },
    time_ms: 1_700_000_000_000,
    ...overrides,
  };
}

/** The whole curve `GET /api/accounts/{address}` serves, oldest first; every other answer carries only the newest point. */
export const FULL_CURVE = [
  { time_ms: 1_700_000_000_000, equity: "10000" },
  { time_ms: 1_700_000_060_000, equity: "10050" },
  { time_ms: 1_700_000_120_000, equity: "10100.5" },
];

/** One account as the API shows it, with the newest curve point only. */
export function viewBody(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    address: ADDRESS,
    state: "running",
    halt: null,
    strategies: [
      {
        id: 0, kind: "sma_crossover", market: "BTC-USD", bar: "1m", running: true,
        params: { fast: 5, slow: 20 }, bounds: { fast: [2, 50], slow: [5, 200] }, warmed_up: true, target: "long",
      },
    ],
    figures: figuresBody(),
    error: null,
    curve: FULL_CURVE.slice(-1),
    ...overrides,
  };
}

/** The totals over the fixtures' one account. */
export function totalsBody(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    accounts: 1,
    reporting: 1,
    balance: "10000",
    equity: "10100.5",
    margin: "50",
    margin_level_bps: 202_100,
    unrealised_pnl: "100.5",
    realised_pnl: "-3.25",
    monkey: { entitled: "5000000000000000000", newest_entitled: "12500000000000000000", claimed: null },
    ...overrides,
  };
}

/** `GET /api/accounts`, and the feed's snapshot besides its type. */
export function overviewBody(accounts: readonly Record<string, unknown>[] = [viewBody()], totals = totalsBody()): Record<string, unknown> {
  return { accounts, totals };
}

/** A fill as the API serves it. */
export function fillBody(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    cursor: "30:0", ts: 1_700_000_000_000, account: ADDRESS, order_id: 1, market: 1, side: "sell", qty: "0.1",
    price: "61000", index: "61000", spread_bps: 0, realised_pnl: "10", profit_fee: "1", position: "0",
    liquidation: false, symbol: "BTC-USD", ...overrides,
  };
}
