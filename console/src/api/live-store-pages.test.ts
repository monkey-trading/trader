/** Tests of what the live store keeps for the accounts whose pages are open: their fills and their curves. */
import type { Outcome } from "./client";
import { ADDRESS, FULL_CURVE, figuresBody, fillBody, overviewBody, totalsBody, viewBody } from "./fixtures";
import { accountMessage, harness, ok } from "./live-store-harness";
import { readAccount, readFills } from "./parsers";
import type { AccountView, Fill } from "./types";

/** The snapshot of a bot that has not yet read its one account. */
const unreadSnapshot = JSON.stringify({ type: "snapshot", ...overviewBody([viewBody({ figures: null })], totalsBody()) });

describe("LiveStore fills of an account the bot has not read", () => {
  it("reads no fills until the account's figures arrive, then reads them", async () => {
    const reads: string[] = [];
    const { store, feeds, settle } = harness({
      fills: (address) => {
        reads.push(address);
        return Promise.resolve(ok<readonly Fill[]>(readFills({ fills: [fillBody()] })));
      },
      account: () => Promise.resolve(ok<AccountView>(readAccount(viewBody({ figures: null })))),
    });
    store.start();
    feeds[0]?.handlers.onOpen();
    feeds[0]?.handlers.onMessage(unreadSnapshot);
    store.track(ADDRESS);
    await settle();
    expect(reads).toEqual([]);
    expect(store.getSnapshot().fills.has(ADDRESS)).toBe(false);

    feeds[0]?.handlers.onMessage(accountMessage("10100"));
    await settle();
    expect(reads).toEqual([ADDRESS]);
    expect(store.getSnapshot().fills.get(ADDRESS)).toHaveLength(1);
  });

  it("reads them when the account read is what brings the figures", async () => {
    const reads: string[] = [];
    const { store, feeds, settle } = harness({
      fills: (address) => {
        reads.push(address);
        return Promise.resolve(ok<readonly Fill[]>([]));
      },
    });
    store.start();
    feeds[0]?.handlers.onOpen();
    feeds[0]?.handlers.onMessage(unreadSnapshot);
    store.track(ADDRESS);
    await settle();
    await settle();

    expect(reads).toEqual([ADDRESS]);
    expect(store.getSnapshot().fills.get(ADDRESS)).toEqual([]);
  });
});

describe("LiveStore curves of accounts whose pages closed", () => {
  const withCurve = { account: () => Promise.resolve(ok<AccountView>(readAccount(viewBody({ curve: FULL_CURVE })))) };

  it("drops an account's curve at the last release of its page, and not before", async () => {
    const { store, settle, open } = harness(withCurve);
    await open();
    const first = store.track(ADDRESS);
    const second = store.track(ADDRESS);
    await settle();
    expect(store.getSnapshot().curves.get(ADDRESS)).toHaveLength(3);

    first();
    await settle();
    expect(store.getSnapshot().curves.get(ADDRESS)).toHaveLength(3);

    second();
    await settle();
    expect(store.getSnapshot().curves.has(ADDRESS)).toBe(false);
  });

  it("does not merge the curve of a read that answers after the release, nor its failure", async () => {
    let finish: (outcome: Outcome<AccountView>) => void = () => undefined;
    const { store, settle, open } = harness({ account: () => new Promise((resolve) => { finish = resolve; }) });
    await open();
    store.track(ADDRESS)();
    finish(ok<AccountView>(readAccount(viewBody({ state: "halted", curve: FULL_CURVE, figures: figuresBody({ equity: "7" }) }))));
    await settle();

    expect(store.getSnapshot().curves.has(ADDRESS)).toBe(false);
    expect(store.getSnapshot().accounts[0]?.figures?.equity).toBe("7");

    store.track(ADDRESS)();
    finish({ status: "unavailable", message: "node down" });
    await settle();
    expect(store.getSnapshot().curveFailures.has(ADDRESS)).toBe(false);
  });
});
