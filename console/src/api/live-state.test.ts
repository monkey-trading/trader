/** Tests of the pure book steps. */
import { detailBody, liveBody, summaryBody } from "./fixtures";
import { CURVE_CAP, EMPTY_BOOK, FILL_CAP, withList, withMessage, withRead } from "./live-state";
import { readAccount, readAccountList, readLiveMessage } from "./parsers";

const ADDRESS = `0x${"11".repeat(20)}`;
const list = (seq: number, ...overrides: Record<string, unknown>[]) =>
  readAccountList({ seq, accounts: overrides.map((o) => summaryBody(o)) });
const read = (seq: number, overrides: Record<string, unknown> = {}) => readAccount({ seq, account: detailBody(overrides) });
const message = (value: unknown) => readLiveMessage(value);
const FILL = { id: "f2", time: 2, market: "BTC-USD", side: "sell", qty: "1", price: "2", fee: "0", realisedPnl: "1" };

describe("withList", () => {
  it("replaces the accounts and drops those the bot no longer lists", () => {
    const first = withList(EMPTY_BOOK, list(1, {}, { address: "0xbeef", name: "b" }));
    const second = withList(withRead(first, read(1)), list(2, {}));
    expect([...second.summaries.keys()]).toEqual([ADDRESS]);
    expect(second.details.has(ADDRESS)).toBe(true);
    expect(withList(second, list(3, { address: "0xbeef" })).details.size).toBe(0);
  });
});

describe("withMessage", () => {
  const held = withRead(EMPTY_BOOK, read(5));

  it("replaces the account's figures and keeps its fills and curve", () => {
    const next = withMessage(held, message({ type: "account", seq: 6, account: liveBody({ equity: "1" }) }));
    expect(next.summaries.get(ADDRESS)?.equity).toBe("1");
    expect(next.details.get(ADDRESS)?.equity).toBe("1");
    expect(next.details.get(ADDRESS)?.fills).toHaveLength(1);
  });

  it("ignores a message at or before the read it follows", () => {
    for (const seq of [4, 5]) {
      expect(withMessage(held, message({ type: "account", seq, account: liveBody({ equity: "1" }) }))).toBe(held);
      expect(withMessage(held, message({ type: "fill", seq, address: ADDRESS, fill: FILL }))).toBe(held);
    }
  });

  it("puts a new fill first and caps the list, dropping the oldest", () => {
    let book = held;
    for (let seq = 6; seq < 6 + FILL_CAP + 10; seq += 1) {
      book = withMessage(book, message({ type: "fill", seq, address: ADDRESS, fill: { ...FILL, id: `n${String(seq)}` } }));
    }
    const fills = book.details.get(ADDRESS)?.fills ?? [];
    expect(fills).toHaveLength(FILL_CAP);
    expect(fills[0]?.id).toBe(`n${String(6 + FILL_CAP + 9)}`);
  });

  it("appends curve points and caps the curve, dropping the oldest", () => {
    let book = held;
    for (let seq = 6; seq < 6 + CURVE_CAP + 5; seq += 1) {
      book = withMessage(book, message({ type: "curve", seq, address: ADDRESS, point: { time: seq, equity: "1" } }));
    }
    const curve = book.details.get(ADDRESS)?.curve ?? [];
    expect(curve).toHaveLength(CURVE_CAP);
    expect(curve.at(-1)?.time).toBe(6 + CURVE_CAP + 4);
  });

  it("changes only the summary of an account whose page is not open", () => {
    const book = withList(EMPTY_BOOK, list(1, {}));
    const next = withMessage(book, message({ type: "account", seq: 2, account: liveBody({ equity: "7" }) }));
    expect(next.summaries.get(ADDRESS)?.equity).toBe("7");
    expect(next.details.size).toBe(0);
    expect(withMessage(book, message({ type: "fill", seq: 2, address: ADDRESS, fill: FILL }))).toBe(book);
  });
});
