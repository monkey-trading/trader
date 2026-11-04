/** Bodies shaped as the bot serves them, for the tests of the parsers, stores and pages. */

/** One account's figures. */
export function summaryBody(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    address: `0x${"11".repeat(20)}`,
    name: "alpha",
    state: "running",
    stateReason: null,
    balance: "10000",
    equity: "10100.5",
    realisedPnl: "-3.25",
    unrealisedPnl: "100.75",
    marginLevel: "33.5",
    openPositions: 1,
    monkeyEarned: "12.5",
    monkeyClaimed: "5",
    ...overrides,
  };
}

/** The lists and strategies of an account. */
export function liveBody(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    ...summaryBody(),
    positions: [{ market: "BTC-USD", qty: "0.1", entry: "60000", mark: "61000", unrealisedPnl: "100", closeout: "48000" }],
    orders: [{ orderId: "7", market: "ETH-USD", side: "buy", qty: "1", price: null, reduceOnly: false }],
    strategies: [
      {
        id: "alpha-sma", kind: "sma_crossover", market: "BTC-USD", bar: "1m", state: "running", reason: null,
        params: [{ name: "fast", kind: "integer", value: "5", min: "2", max: "50" }],
      },
    ],
    ...overrides,
  };
}

/** An account's whole page. */
export function detailBody(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    ...liveBody(),
    fills: [
      { id: "f1", time: 1_700_000_000_000, market: "BTC-USD", side: "buy", qty: "0.1", price: "60000", fee: "0.5", realisedPnl: "0" },
    ],
    curve: [
      { time: 1_700_000_000_000, equity: "10000" },
      { time: 1_700_000_060_000, equity: "10100.5" },
    ],
    ...overrides,
  };
}
