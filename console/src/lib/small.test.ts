/** Tests of the small pure helpers: reconnect delay, routes, totals, curve layout, parameter checks. */
import { BACKOFF_BASE_MS, BACKOFF_CAP_MS, reconnectDelay } from "./backoff";
import { curvePath, curveRange } from "./curve";
import { checkParams } from "./params";
import { accountPath, parseRoute } from "./route";
import { totalsOf } from "./totals";

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

describe("totalsOf", () => {
  const row = (n: string) => ({
    equity: n, realisedPnl: n, unrealisedPnl: `-${n}`, openPositions: 1, monkeyEarned: n, monkeyClaimed: "0",
  });

  it("sums served strings exactly", () => {
    expect(totalsOf([row("0.1"), row("0.2")])).toEqual({
      equity: "0.3", realisedPnl: "0.3", unrealisedPnl: "-0.3", openPositions: 2, monkeyEarned: "0.3", monkeyClaimed: "0",
    });
  });

  it("totals no accounts as zero", () => {
    expect(totalsOf([]).equity).toBe("0");
  });
});

describe("curve", () => {
  const points = [
    { time: 0, equity: "100" },
    { time: 10, equity: "150" },
    { time: 20, equity: "50.5" },
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
  const sma = [
    { name: "fast", kind: "integer", draft: "5", min: "2", max: "50" },
    { name: "width", kind: "decimal", draft: " 1.5 ", min: "0.5", max: "4" },
  ] as const;

  it("sends integers as numbers and decimals as strings", () => {
    expect(checkParams(sma)).toEqual({ status: "ok", values: { fast: 5, width: "1.5" } });
  });

  it("names each problem at its field and sends nothing", () => {
    const result = checkParams([
      { ...sma[0], draft: "1" },
      { ...sma[1], draft: "2.5.1" },
    ]);
    expect(result).toEqual({
      status: "invalid",
      problems: { fast: "Enter a value from 2 to 50.", width: "Enter a decimal number." },
    });
  });

  it("refuses a fraction for an integer", () => {
    expect(checkParams([{ ...sma[0], draft: "5.5" }])).toEqual({
      status: "invalid",
      problems: { fast: "Enter a whole number." },
    });
  });
});
