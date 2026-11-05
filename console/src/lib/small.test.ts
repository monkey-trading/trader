/** Tests of the small pure helpers: reconnect delay, routes, totals, curve layout, parameter checks. */
import { BACKOFF_BASE_MS, BACKOFF_CAP_MS, reconnectDelay } from "./backoff";
import { curvePath, curveRange } from "./curve";
import { checkParams } from "./params";
import { accountPath, parseRoute } from "./route";
import { marketName, symbolsOf } from "./markets";

describe("reconnectDelay", () => {
  it("doubles from the base, with jitter between half and all of it", () => {
    expect(reconnectDelay(0, 0)).toBe(BACKOFF_BASE_MS / 2);
    expect(reconnectDelay(0, 0.999)).toBe(499);
    expect(reconnectDelay(2, 0)).toBe(1000);
  });

  it("stops growing at the cap", () => {
    expect(reconnectDelay(40, 0)).toBe(BACKOFF_CAP_MS / 2);
  });
});

describe("parseRoute", () => {
  const address = `0x${"ab".repeat(20)}`;

  it("names the overview and an account page", () => {
    expect(parseRoute("/")).toEqual({ name: "overview" });
    expect(parseRoute(accountPath(address))).toEqual({ name: "account", address });
    expect(parseRoute(`${accountPath(address)}/`)).toEqual({ name: "account", address });
  });

  it("refuses a malformed address and unknown paths", () => {
    expect(parseRoute("/accounts/0x12")).toEqual({ name: "not-found" });
    expect(parseRoute("/accounts/../etc")).toEqual({ name: "not-found" });
    expect(parseRoute("/other")).toEqual({ name: "not-found" });
  });
});

describe("market names", () => {
  it("takes symbols from positions and fills and falls back to the id", () => {
    const symbols = symbolsOf([{ market: 1, symbol: "BTC-USD" }], [{ market: 2, symbol: null }, { market: 3, symbol: "ETH-USD" }]);
    expect([marketName(1, symbols), marketName(2, symbols), marketName(3, symbols)]).toEqual(["BTC-USD", "Market 2", "ETH-USD"]);
  });
});

describe("curve", () => {
  const points = [
    { timeMs: 0, equity: "100" },
    { timeMs: 10, equity: "150" },
    { timeMs: 20, equity: "50.5" },
  ];

  it("reports the range as exact two-place strings", () => {
    expect(curveRange(points)).toEqual({ low: "50.50", high: "150.00", latest: "50.50" });
    expect(curveRange([])).toBeNull();
  });

  it("scales time across the width and equity across the height, high at the top", () => {
    expect(curvePath(points)).toBe("0.0,80.4 300.0,0.0 600.0,160.0");
  });

  it("draws nothing for fewer than two points", () => {
    expect(curvePath(points.slice(0, 1))).toBe("");
  });
});

describe("checkParams", () => {
  const fast = { name: "fast", draft: "5", bounds: [2, 50] } as const;
  const slow = { name: "slow", draft: " 20 ", bounds: [5, 200] } as const;

  it("sends whole numbers, trimmed", () => {
    expect(checkParams([fast, slow])).toEqual({ status: "ok", values: { fast: 5, slow: 20 } });
  });

  it("accepts the bounds themselves and refuses one step beyond", () => {
    expect(checkParams([{ ...fast, draft: "2" }, { ...slow, draft: "200" }]).status).toBe("ok");
    expect(checkParams([{ ...fast, draft: "1" }, { ...slow, draft: "201" }])).toEqual({
      status: "invalid",
      problems: { fast: "Enter a value from 2 to 50.", slow: "Enter a value from 5 to 200." },
    });
  });

  it("names each problem at its field and sends nothing", () => {
    expect(checkParams([{ ...fast, draft: "5.5" }, { ...slow, draft: "-3" }, { name: "x", draft: "", bounds: [1, 2] }])).toEqual({
      status: "invalid",
      problems: { fast: "Enter a whole number.", slow: "Enter a whole number.", x: "Enter a whole number." },
    });
  });
});
