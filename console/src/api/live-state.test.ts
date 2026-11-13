/** Tests of the pure book steps. */
import { ADDRESS, FULL_CURVE, failedBody, fillBody, overviewBody, totalsBody, viewBody } from "./fixtures";
import { CURVE_CAP, CURVE_WINDOW_MS, EMPTY_BOOK, FILL_CAP, identityOf, keyOf, mergeCurve, withAccount, withMessage, withSnapshot } from "./live-state";
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

  it("holds an account with no address under its label, in the bot's order, and replaces it in place", () => {
    const book = withSnapshot(EMPTY_BOOK, overview(failedBody(), viewBody(), failedBody({ label: "account_dir/b.json" })));
    expect([...book.accounts.keys()]).toEqual(["label:keys/locked.json", ADDRESS, "label:account_dir/b.json"]);

    const next = withMessage(book, readLiveMessage({ type: "account", account: failedBody({ halt: "still locked" }) }));
    expect([...next.accounts.values()].map((a) => a.halt)).toEqual(["still locked", null, "the key would not unlock"]);
    expect(identityOf({ address: null, label: "x", code: "key_locked" })).toBe("label:x");
  });

  it("keeps an entry that could not start apart from the account that trades under the same address", () => {
    const refused = viewBody({ code: "account_refused", halt: "listed twice", figures: null, strategies: [], state: "halted" });
    const book = withSnapshot(EMPTY_BOOK, overview(viewBody(), refused));
    expect([...book.accounts.keys()]).toEqual([ADDRESS, "failed:1"]);
    expect(book.accounts.get(ADDRESS)?.state).toBe("running");
  });

  it("replaces the entry that could not start with the full read of it, never adding or taking another's place", () => {
    const refused = (address: string, code: string) =>
      viewBody({ address, code, halt: code, figures: null, strategies: [], state: "halted" });
    const book = withSnapshot(EMPTY_BOOK, overview(refused(OTHER, "no_balance"), viewBody(), refused(ADDRESS.replace("1", "2"), "no_balance")));
    const third = readLiveMessage({ type: "account", account: { ...refused(ADDRESS.replace("1", "2"), "no_balance"), halt: "read again" } });
    const next = withMessage(book, third);
    expect([...next.accounts.keys()]).toEqual(["failed:0", ADDRESS, "failed:2"]);
    expect([...next.accounts.values()].map((a) => a.halt)).toEqual(["no_balance", null, "read again"]);
    // A failed entry the snapshot did not list is not added.
    const stranger = readLiveMessage({ type: "account", account: refused("0x3333333333333333333333333333333333333333", "no_balance") });
    expect(withMessage(book, stranger).accounts.size).toBe(3);
  });

  it("keeps the fills of an account that stays", () => {
    const held = withMessage(withSnapshot(EMPTY_BOOK, overview(viewBody())), { type: "fills", address: ADDRESS, fills: fills(1) });
    expect(withSnapshot(held, overview(viewBody())).fills.get(ADDRESS)).toHaveLength(1);
  });

  it("starts an account's curve from the newest point the snapshot carries and keeps one it already had", () => {
    const first = withSnapshot(EMPTY_BOOK, overview(viewBody({ curve: [{ time_ms: 60_000, equity: "1" }] })));
    const second = withSnapshot(first, overview(viewBody({ curve: [{ time_ms: 120_000, equity: "2" }] })));
    expect(first.curves.get(ADDRESS)).toEqual([{ timeMs: 60_000, equity: "1" }]);
    expect(second.curves.get(ADDRESS)?.map((p) => p.timeMs)).toEqual([60_000, 120_000]);
    expect(withSnapshot(second, overview(viewBody({ address: OTHER }))).curves.has(ADDRESS)).toBe(false);
  });
});

describe("mergeCurve", () => {
  const MINUTE = 60_000;
  const point = (timeMs: number, equity = "1"): CurvePoint => ({ timeMs, equity });

  it("keeps one point per minute as the bot does, the latest in it winning, oldest first", () => {
    const merged = mergeCurve([point(2 * MINUTE + 5, "old"), point(MINUTE)], [point(3 * MINUTE), point(2 * MINUTE + 9, "new")]);
    expect(merged).toEqual([point(MINUTE), point(2 * MINUTE + 9, "new"), point(3 * MINUTE)]);
  });

  it("does not let a stale answer replace a newer point of the same minute", () => {
    expect(mergeCurve([point(MINUTE + 30, "newer")], [point(MINUTE + 10, "older")])).toEqual([point(MINUTE + 30, "newer")]);
  });

  it("holds the day's history through many reads a minute: the feed's points do not crowd it out", () => {
    const day = Array.from({ length: 24 * 60 }, (_, i) => point(i * MINUTE));
    let curve = mergeCurve([], day);
    const start = 24 * 60 * MINUTE;
    for (let read = 0; read < 3000; read += 1) {
      curve = mergeCurve(curve, [point(start + read * 5_000, String(read))]);
    }
    // 3,000 reads at 5 s apart span 250 minutes: the curve keeps one point per minute within the window.
    expect(curve.length).toBeLessThanOrEqual(CURVE_CAP);
    expect(curve[0]?.timeMs).toBeGreaterThanOrEqual((start + 2999 * 5_000) - CURVE_WINDOW_MS);
    expect(curve.length).toBeGreaterThan(24 * 60 - 260);
  });

  it("returns the held curve itself when nothing arrives", () => {
    const held = [point(1)];
    expect(mergeCurve(held, [])).toBe(held);
  });

  it("trims to 24 hours before the newest point, the boundary point staying", () => {
    const newest = 10 * CURVE_WINDOW_MS;
    const merged = mergeCurve([point(newest - CURVE_WINDOW_MS - MINUTE), point(newest - CURVE_WINDOW_MS)], [point(newest)]);
    expect(merged.map((p) => p.timeMs)).toEqual([newest - CURVE_WINDOW_MS, newest]);
  });

  it("keeps a day of minutes, within the cap, dropping the older", () => {
    const merged = mergeCurve([], Array.from({ length: CURVE_CAP + 3 }, (_, i) => point(i * MINUTE)));
    expect(merged.length).toBeLessThanOrEqual(CURVE_CAP);
    expect(merged.length).toBe(24 * 60 + 1);
    expect(merged.at(-1)?.timeMs).toBe((CURVE_CAP + 2) * MINUTE);
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
