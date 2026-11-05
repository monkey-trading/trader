/** Tests of the pure book steps. */
import { ADDRESS, fillBody, overviewBody, totalsBody, viewBody } from "./fixtures";
import { CURVE_CAP, EMPTY_BOOK, FILL_CAP, keyOf, withMessage, withSnapshot } from "./live-state";
import { readAccount, readFills, readLiveMessage, readOverview } from "./parsers";

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

  it("caps an account's curve to the newest points", () => {
    const long = Array.from({ length: CURVE_CAP + 3 }, (_, i) => ({ time_ms: i, equity: "1" }));
    const book = withSnapshot(EMPTY_BOOK, overview(viewBody({ curve: long })));
    const curve = book.accounts.get(ADDRESS)?.curve ?? [];
    expect([curve.length, curve[0]?.timeMs]).toEqual([CURVE_CAP, 3]);
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

  it("takes a snapshot message as a snapshot", () => {
    const next = withMessage(held, readLiveMessage({ type: "snapshot", ...overviewBody([viewBody()]) }));
    expect(next.accounts.size).toBe(1);
    expect(readAccount(viewBody()).address).toBe(ADDRESS);
  });
});
