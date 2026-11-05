/** Tests of the pure book steps. */
import { ADDRESS, FULL_CURVE, fillBody, overviewBody, totalsBody, viewBody } from "./fixtures";
import { CURVE_CAP, CURVE_WINDOW_MS, EMPTY_BOOK, FILL_CAP, keyOf, mergeCurve, withAccount, withMessage, withSnapshot } from "./live-state";
import { readAccount, readFills, readLiveMessage, readOverview } from "./parsers";
import type { CurvePoint } from "./types";

const OTHER = `0x${"22".repeat(20)}`;
const overview = (...accounts: Record<string, unknown>[]) => readOverview(overviewBody(accounts, totalsBody()));
const fills = (count: number) => readFills({ fills: Array.from({ length: count }, (_, i) => fillBody({ cursor: `${String(i)}:0` })) });

describe("withSnapshot", () => {
  it("replaces the accounts and totals, keeps the order the bot gave and drops fills of accounts that went", () => {
    const first = withSnapshot(EMPTY_BOOK, overview(viewBody(), viewBody({ address: OTHER })));
    const held = withMessage(first, { type: "fills", address: OTHER, fills: fills(2) });
    const second = withSnapshot(held, overview(viewBody()));

    expect([...first.accounts.keys()]).toEqual([ADDRESS, OTHER]);
    expect([...second.accounts.keys()]).toEqual([ADDRESS]);
    expect(second.fills.size).toBe(0);
    expect(held.fills.get(OTHER)).toHaveLength(2);
  });

  it("keeps the fills of an account that stays", () => {
    const held = withMessage(withSnapshot(EMPTY_BOOK, overview(viewBody())), { type: "fills", address: ADDRESS, fills: fills(1) });
    expect(withSnapshot(held, overview(viewBody())).fills.get(ADDRESS)).toHaveLength(1);
  });

  it("starts an account's curve from the newest point the snapshot carries and keeps one it already had", () => {
    const first = withSnapshot(EMPTY_BOOK, overview(viewBody({ curve: [{ time_ms: 1000, equity: "1" }] })));
    const second = withSnapshot(first, overview(viewBody({ curve: [{ time_ms: 2000, equity: "2" }] })));
    expect(first.curves.get(ADDRESS)).toEqual([{ timeMs: 1000, equity: "1" }]);
    expect(second.curves.get(ADDRESS)?.map((p) => p.timeMs)).toEqual([1000, 2000]);
    expect(withSnapshot(second, overview(viewBody({ address: OTHER }))).curves.has(ADDRESS)).toBe(false);
  });
});

describe("mergeCurve", () => {
  const point = (timeMs: number, equity = "1"): CurvePoint => ({ timeMs, equity });

  it("keeps one point per time, the incoming one winning, oldest first", () => {
    expect(mergeCurve([point(2, "old"), point(1)], [point(3), point(2, "new")])).toEqual([point(1), point(2, "new"), point(3)]);
  });

  it("returns the held curve itself when nothing arrives", () => {
    const held = [point(1)];
    expect(mergeCurve(held, [])).toBe(held);
  });

  it("trims to 24 hours before the newest point, the boundary point staying", () => {
    const newest = 10 * CURVE_WINDOW_MS;
    const merged = mergeCurve([point(newest - CURVE_WINDOW_MS - 1), point(newest - CURVE_WINDOW_MS)], [point(newest)]);
    expect(merged.map((p) => p.timeMs)).toEqual([newest - CURVE_WINDOW_MS, newest]);
  });

  it("keeps at most the cap, dropping the oldest", () => {
    const merged = mergeCurve([], Array.from({ length: CURVE_CAP + 3 }, (_, i) => point(i)));
    expect([merged.length, merged[0]?.timeMs]).toEqual([CURVE_CAP, 3]);
  });
});

describe("withMessage", () => {
  const held = withSnapshot(EMPTY_BOOK, overview(viewBody(), viewBody({ address: OTHER })));

  it("replaces one account's view in place, without changing the order", () => {
    const next = withMessage(held, readLiveMessage({ type: "account", account: viewBody({ state: "stopped" }) }));
    expect([...next.accounts.values()].map((a) => a.state)).toEqual(["stopped", "running"]);
  });

  it("holds an account's address in either case under one key", () => {
    const next = withMessage(held, readLiveMessage({ type: "account", account: viewBody({ address: ADDRESS.toUpperCase().replace("0X", "0x") }) }));
    expect(next.accounts.size).toBe(2);
    expect(keyOf("0xAB")).toBe("0xab");
  });

  it("replaces fills, newest first, capped, and ignores fills of an unlisted account", () => {
    const next = withMessage(held, { type: "fills", address: ADDRESS, fills: fills(FILL_CAP + 10) });
    expect(next.fills.get(ADDRESS)).toHaveLength(FILL_CAP);
    expect(next.fills.get(ADDRESS)?.[0]?.cursor).toBe("0:0");
    expect(withMessage(held, { type: "fills", address: "0xdead", fills: fills(1) })).toBe(held);
  });

  it("takes new totals", () => {
    const next = withMessage(held, readLiveMessage({ type: "totals", totals: totalsBody({ equity: "7" }) }));
    expect(next.totals?.equity).toBe("7");
  });

  it("appends the point of an account message to the curve, once however often it repeats", () => {
    const message = readLiveMessage({ type: "account", account: viewBody({ curve: [{ time_ms: 1_700_000_180_000, equity: "9" }] }) });
    const once = withMessage(held, message);
    const twice = withMessage(once, message);
    expect(twice.curves.get(ADDRESS)?.map((p) => p.timeMs)).toEqual([1_700_000_120_000, 1_700_000_180_000]);
    expect(twice.curves.get(ADDRESS)).toHaveLength(once.curves.get(ADDRESS)?.length ?? -1);
  });

  it("merges a full read's curve with the points the feed brought before it", () => {
    const full = readAccount(viewBody({ curve: FULL_CURVE }));
    const next = withAccount(withMessage(held, readLiveMessage({ type: "account", account: viewBody({ curve: [{ time_ms: 1_700_000_180_000, equity: "10200" }] }) })), full);
    expect(next.curves.get(ADDRESS)?.map((p) => p.timeMs)).toEqual([1_700_000_000_000, 1_700_000_060_000, 1_700_000_120_000, 1_700_000_180_000]);
  });

  it("takes a snapshot message as a snapshot", () => {
    const next = withMessage(held, readLiveMessage({ type: "snapshot", ...overviewBody([viewBody()]) }));
    expect(next.accounts.size).toBe(1);
    expect(readAccount(viewBody()).address).toBe(ADDRESS);
  });
});
