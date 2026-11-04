/** Tests of the parsers at the API boundary. */
import { detailBody, liveBody, summaryBody } from "./fixtures";
import { parseWith, readAccount, readAccountList, readErrorMessage, readLiveMessage } from "./parsers";

const FILL = { id: "f1", time: 1, market: "BTC-USD", side: "buy", qty: "1", price: "2", fee: "0", realisedPnl: "0" };

describe("readAccountList", () => {
  it("reads a valid list whole", () => {
    const parsed = parseWith(readAccountList, { seq: 4, accounts: [summaryBody()] });
    expect(parsed).toEqual({
      status: "ok",
      value: {
        seq: 4,
        accounts: [
          {
            address: `0x${"11".repeat(20)}`, name: "alpha", state: "running", stateReason: null, balance: "10000",
            equity: "10100.5", realisedPnl: "-3.25", unrealisedPnl: "100.75", marginLevel: "33.5", openPositions: 1,
            monkeyEarned: "12.5", monkeyClaimed: "5",
          },
        ],
      },
    });
  });

  it("refuses a missing field and names it", () => {
    const partial = Object.fromEntries(Object.entries(summaryBody()).filter(([key]) => key !== "equity"));
    expect(parseWith(readAccountList, { seq: 1, accounts: [partial] })).toEqual({
      status: "invalid", message: "equity is not a text",
    });
  });

  it("refuses wrong types: a float amount, a negative count, an unknown state", () => {
    const bad = (overrides: Record<string, unknown>) => parseWith(readAccountList, { seq: 1, accounts: [summaryBody(overrides)] });
    expect(bad({ equity: 10100.5 }).status).toBe("invalid");
    expect(bad({ equity: "1e5" }).status).toBe("invalid");
    expect(bad({ openPositions: -1 }).status).toBe("invalid");
    expect(bad({ state: "paused" }).status).toBe("invalid");
  });

  it("drops fields it does not know", () => {
    const parsed = parseWith(readAccountList, { seq: 1, accounts: [summaryBody({ privateKey: "0xsecret" })], extra: 1 });
    expect(JSON.stringify(parsed)).not.toContain("secret");
  });

  it("refuses a body that is not an object", () => {
    expect(parseWith(readAccountList, [])).toEqual({ status: "invalid", message: "accounts body is not an object" });
  });
});

describe("readAccount", () => {
  it("reads positions, orders, strategies, fills and the curve", () => {
    const parsed = parseWith(readAccount, { seq: 9, account: detailBody() });
    expect(parsed.status).toBe("ok");
    if (parsed.status === "ok") {
      expect(parsed.value.account.positions[0]?.closeout).toBe("48000");
      expect(parsed.value.account.orders[0]?.price).toBeNull();
      expect(parsed.value.account.strategies[0]?.params[0]).toEqual({ name: "fast", kind: "integer", value: "5", min: "2", max: "50" });
      expect(parsed.value.account.fills).toHaveLength(1);
      expect(parsed.value.account.curve).toHaveLength(2);
    }
  });

  it("refuses an order side the console does not know", () => {
    const body = detailBody({ orders: [{ orderId: "1", market: "X", side: "hold", qty: "1", price: null, reduceOnly: false }] });
    expect(parseWith(readAccount, { seq: 1, account: body }).status).toBe("invalid");
  });
});

describe("readLiveMessage", () => {
  it("reads each of the three message types", () => {
    expect(readLiveMessage({ type: "account", seq: 2, account: liveBody() }).type).toBe("account");
    expect(readLiveMessage({ type: "curve", seq: 3, address: "0xa", point: { time: 1, equity: "5" } })).toEqual({
      type: "curve", seq: 3, address: "0xa", point: { time: 1, equity: "5" },
    });
    expect(readLiveMessage({ type: "fill", seq: 4, address: "0xa", fill: FILL }).type).toBe("fill");
  });

  it("throws on an unknown type so the store can count it", () => {
    expect(parseWith(readLiveMessage, { type: "delete", seq: 1 }).status).toBe("invalid");
    expect(parseWith(readLiveMessage, { type: "curve", address: "0xa" }).status).toBe("invalid");
  });
});

describe("readErrorMessage", () => {
  it("takes the error text and ignores any other body", () => {
    expect(readErrorMessage({ error: "no" })).toBe("no");
    expect(readErrorMessage({ message: "no" })).toBeNull();
    expect(readErrorMessage(null)).toBeNull();
  });
});
