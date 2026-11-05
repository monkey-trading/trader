/** Tests of the parsers at the API boundary. */
import { fillBody, figuresBody, overviewBody, totalsBody, viewBody } from "./fixtures";
import { parseWith, readAccount, readErrorMessage, readFills, readLiveMessage, readOverview } from "./parsers";

describe("readOverview", () => {
  it("reads accounts and totals whole", () => {
    const parsed = parseWith(readOverview, overviewBody());
    expect(parsed.status).toBe("ok");
    if (parsed.status === "ok") {
      expect(parsed.value.totals).toEqual({
        accounts: 1, reporting: 1, balance: "10000", equity: "10100.5", margin: "50", marginLevelBps: 202_100,
        unrealisedPnl: "100.5", realisedPnl: "-3.25",
        monkey: { entitled: "5000000000000000000", newestEntitled: "12500000000000000000", claimed: null },
      });
      expect(parsed.value.accounts[0]).toEqual({
        address: `0x${"11".repeat(20)}`,
        state: "running",
        halt: null,
        strategies: [
          {
            id: 0, kind: "sma_crossover", market: "BTC-USD", bar: "1m", running: true, warmedUp: true, target: "long",
            params: [
              { name: "fast", value: 5, bounds: [2, 50] },
              { name: "slow", value: 20, bounds: [5, 200] },
            ],
          },
        ],
        figures: {
          balance: "10000", equity: "10100.5", margin: "50", marginLevelBps: 202_100, unrealisedPnl: "100.5",
          realisedPnl: "-3.25", timeMs: 1_700_000_000_000,
          positions: [
            { market: 1, symbol: "BTC-USD", qty: "0.1", entry: "60000", mark: "61000", unrealisedPnl: "100", closeout: "48000" },
          ],
          orders: [{ orderId: 9, market: 2, side: "buy", qty: "1", kind: { type: "limit", price: "50" }, reduceOnly: false }],
          monkey: { entitled: "5000000000000000000", newestEntitled: "12500000000000000000", claimed: null },
        },
        error: null,
        curve: [
          { timeMs: 1_700_000_000_000, equity: "10000" },
          { timeMs: 1_700_000_060_000, equity: "10100.5" },
        ],
      });
    }
  });

  it("reads an account with no figures yet and the reason", () => {
    const parsed = parseWith(readAccount, viewBody({ figures: null, error: "the node did not answer" }));
    expect(parsed.status === "ok" && [parsed.value.figures, parsed.value.error]).toEqual([null, "the node did not answer"]);
  });

  it("reads strategies whose periods the console may not set", () => {
    const parsed = parseWith(
      readAccount,
      viewBody({
        strategies: [{ id: 0, kind: "sma_crossover", market: "BTC-USD", bar: "1m", running: false, params: { fast: 5, slow: 20 }, bounds: null, warmed_up: false, target: null }],
      }),
    );
    expect(parsed.status === "ok" && parsed.value.strategies[0]?.params).toEqual([
      { name: "fast", value: 5, bounds: null },
      { name: "slow", value: 20, bounds: null },
    ]);
  });

  it("refuses a missing field and names it", () => {
    const partial = Object.fromEntries(Object.entries(figuresBody()).filter(([key]) => key !== "equity"));
    expect(parseWith(readAccount, viewBody({ figures: partial }))).toEqual({ status: "invalid", message: "equity is not a text" });
  });

  it("refuses wrong types: a float amount, an exponent, an unknown state, MONKEY with a point", () => {
    const bad = (overrides: Record<string, unknown>) => parseWith(readAccount, viewBody(overrides));
    expect(bad({ figures: figuresBody({ equity: 10100.5 }) }).status).toBe("invalid");
    expect(bad({ figures: figuresBody({ equity: "1e5" }) }).status).toBe("invalid");
    expect(bad({ state: "paused" }).status).toBe("invalid");
    expect(bad({ figures: figuresBody({ monkey: { entitled: "1.5", newest_entitled: "1", claimed: null } }) }).status).toBe("invalid");
    expect(bad({ figures: figuresBody({ margin_level_bps: 1.5 }) }).status).toBe("invalid");
  });

  it("drops fields it does not know, so nothing unexpected reaches the page", () => {
    const parsed = parseWith(readOverview, overviewBody([viewBody({ private_key: "0xsecret" })], totalsBody({ extra: 1 })));
    expect(JSON.stringify(parsed)).not.toContain("secret");
  });

  it("refuses a body that is not an object", () => {
    expect(parseWith(readOverview, [])).toEqual({ status: "invalid", message: "accounts body is not an object" });
  });
});

describe("readFills", () => {
  it("reads fills with their cursor, time and profit fee", () => {
    const parsed = parseWith(readFills, { fills: [fillBody()] });
    expect(parsed).toEqual({
      status: "ok",
      value: [
        { cursor: "30:0", time: 1_700_000_000_000, market: 1, symbol: "BTC-USD", side: "sell", qty: "0.1", price: "61000", realisedPnl: "10", profitFee: "1" },
      ],
    });
  });

  it("reads a fill whose market has no symbol", () => {
    const parsed = parseWith(readFills, { fills: [fillBody({ symbol: null })] });
    expect(parsed.status === "ok" && parsed.value[0]?.symbol).toBeNull();
  });
});

describe("readLiveMessage", () => {
  it("reads each of the four message types", () => {
    expect(readLiveMessage({ type: "snapshot", ...overviewBody() }).type).toBe("snapshot");
    expect(readLiveMessage({ type: "account", account: viewBody() }).type).toBe("account");
    expect(readLiveMessage({ type: "fills", address: ADDRESS_A, fills: [fillBody()] }).type).toBe("fills");
    expect(readLiveMessage({ type: "totals", totals: totalsBody() }).type).toBe("totals");
  });

  it("refuses an unknown type and a message with a missing part", () => {
    expect(parseWith(readLiveMessage, { type: "delete" }).status).toBe("invalid");
    expect(parseWith(readLiveMessage, { type: "fills", fills: [] }).status).toBe("invalid");
  });
});

describe("readErrorMessage", () => {
  it("takes the error text and ignores any other body", () => {
    expect(readErrorMessage({ error: "no" })).toBe("no");
    expect(readErrorMessage({ message: "no" })).toBeNull();
    expect(readErrorMessage(null)).toBeNull();
  });
});

const ADDRESS_A = `0x${"11".repeat(20)}`;
