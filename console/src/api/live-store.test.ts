/** Tests of the live store over a stand-in feed, API and clock. */
import type { Outcome } from "./client";
import { ADDRESS, FULL_CURVE, fillBody, viewBody } from "./fixtures";
import { accountMessage, harness, ok, snapshot } from "./live-store-harness";
import { readAccount, readFills } from "./parsers";
import type { AccountView, Fill } from "./types";

describe("LiveStore", () => {
  it("is live once the snapshot has arrived, and not before", async () => {
    const { store, feeds, settle, flush } = harness();
    store.start();
    flush();
    expect(store.getSnapshot().connection).toBe("connecting");
    feeds[0]?.handlers.onOpen();
    await settle();
    expect(store.getSnapshot()).toMatchObject({ connection: "syncing", loaded: false });

    feeds[0]?.handlers.onMessage(snapshot());
    await settle();

    expect(store.getSnapshot().connection).toBe("live");
    expect(store.getSnapshot().accounts.map((a) => a.figures?.equity)).toEqual(["10100.5"]);
    expect(store.getSnapshot().totals?.equity).toBe("10100.5");
  });

  it("counts a message before the snapshot, and one that fails its check, and shows nothing of them", async () => {
    const { store, feeds, settle } = harness();
    store.start();
    feeds[0]?.handlers.onOpen();
    feeds[0]?.handlers.onMessage(accountMessage("1"));
    feeds[0]?.handlers.onMessage(snapshot());
    feeds[0]?.handlers.onMessage("not json");
    feeds[0]?.handlers.onMessage(accountMessage("5e3"));
    await settle();

    expect(store.getSnapshot().dropped).toBe(3);
    expect(store.getSnapshot().accounts.map((a) => a.figures?.equity)).toEqual(["10100.5"]);
  });

  it("applies later messages and publishes once per frame however many arrive", async () => {
    const { store, feeds, flush, open } = harness();
    await open();
    let published = 0;
    store.subscribe(() => { published += 1; });
    for (let n = 1; n <= 50; n += 1) {
      feeds[0]?.handlers.onMessage(accountMessage(String(n)));
    }
    flush();

    expect(published).toBe(1);
    expect(store.getSnapshot().accounts[0]?.figures?.equity).toBe("50");
  });

  it("checks the session after a close, then reconnects with a capped, jittered wait and a fresh snapshot", async () => {
    const { store, feeds, timers, settle, flush, open } = harness();
    await open();

    feeds[0]?.handlers.onClose();
    feeds[0]?.handlers.onClose();
    await settle();
    flush();
    expect(store.getSnapshot().connection).toBe("reconnecting");
    expect(timers).toHaveLength(1);
    expect(timers[0]?.ms).toBe(250);

    timers[0]?.callback();
    expect(feeds).toHaveLength(2);
    feeds[1]?.handlers.onOpen();
    feeds[1]?.handlers.onMessage(snapshot("1"));
    await settle();
    expect(store.getSnapshot()).toMatchObject({ connection: "live" });
    expect(store.getSnapshot().accounts[0]?.figures?.equity).toBe("1");
  });

  it("ends the session instead of retrying when the session check says 401", async () => {
    const { store, feeds, ended, timers, settle, open } = harness({ session: () => Promise.resolve({ status: "unauthenticated" }) });
    await open();
    feeds[0]?.handlers.onClose();
    await settle();

    expect(ended).toEqual(["ended"]);
    expect(timers).toHaveLength(0);
    expect(store.getSnapshot()).toMatchObject({ connection: "stopped", accounts: [] });
  });

  it("ignores the late events of a connection that a stop and start replaced", async () => {
    const { store, feeds, settle } = harness();
    const stop = store.start();
    stop();
    store.start();
    feeds[0]?.handlers.onOpen();
    feeds[0]?.handlers.onMessage(snapshot("666"));
    await settle();

    expect(store.getSnapshot().connection).toBe("connecting");
    expect(feeds).toHaveLength(2);
    expect(feeds[0]?.closed).toBe(true);
  });

  it("forgets every account on stop", async () => {
    const { store, settle, open } = harness();
    const stop = await open();
    expect(store.getSnapshot().accounts).toHaveLength(1);
    stop();
    await settle();

    expect(store.getSnapshot()).toMatchObject({ connection: "stopped", loaded: false, accounts: [], totals: null });
  });

  it("reads a tracked account's fills, and again after a new snapshot", async () => {
    const reads: string[] = [];
    const { store, feeds, settle } = harness({
      fills: (address) => {
        reads.push(address);
        return Promise.resolve(ok<readonly Fill[]>(readFills({ fills: [fillBody()] })));
      },
    });
    store.start();
    feeds[0]?.handlers.onOpen();
    store.track(ADDRESS);
    feeds[0]?.handlers.onMessage(snapshot());
    await settle();
    await settle();

    expect(reads).toEqual([ADDRESS, ADDRESS]);
    expect(store.getSnapshot().fills.get(ADDRESS)?.map((f) => f.cursor)).toEqual(["30:0"]);
  });

  it("prefers a feed fills message over a read that began before it", async () => {
    let finish: (outcome: Outcome<readonly Fill[]>) => void = () => undefined;
    const { store, feeds, settle, open } = harness({ fills: () => new Promise((resolve) => { finish = resolve; }) });
    await open();
    store.track(ADDRESS);
    feeds[0]?.handlers.onMessage(JSON.stringify({ type: "fills", address: ADDRESS, fills: [fillBody({ cursor: "99:0" })] }));
    finish(ok<readonly Fill[]>(readFills({ fills: [fillBody({ cursor: "1:0" })] })));
    await settle();

    expect(store.getSnapshot().fills.get(ADDRESS)?.map((f) => f.cursor)).toEqual(["99:0"]);
  });

  it("shows a control's result from a fresh read, unless the feed has spoken since", async () => {
    let state: "stopped" | "running" = "stopped";
    const { store, feeds, settle, open } = harness({
      account: () => Promise.resolve(ok<AccountView>(readAccount(viewBody({ state })))),
    });
    await open();
    await store.refresh(ADDRESS);
    await settle();
    expect(store.getSnapshot().accounts[0]?.state).toBe("stopped");

    state = "running";
    const pending = store.refresh(ADDRESS);
    feeds[0]?.handlers.onMessage(JSON.stringify({ type: "account", account: viewBody({ state: "halted" }) }));
    await pending;
    await settle();
    expect(store.getSnapshot().accounts[0]?.state).toBe("halted");
  });

  it("holds no curve for an account no page shows, reads the whole curve when its page opens, then adds each feed point once", async () => {
    const { store, feeds, settle, open } = harness({
      account: () => Promise.resolve(ok<AccountView>(readAccount(viewBody({ curve: FULL_CURVE })))),
    });
    await open();
    expect(store.getSnapshot().curves.has(ADDRESS)).toBe(false);

    store.track(ADDRESS);
    await settle();
    const point = (time_ms: number) => JSON.stringify({ type: "account", account: viewBody({ curve: [{ time_ms, equity: "7" }] }) });
    feeds[0]?.handlers.onMessage(point(1_700_000_180_000));
    feeds[0]?.handlers.onMessage(point(1_700_000_180_000));
    await settle();

    expect(store.getSnapshot().curves.get(ADDRESS)?.map((p) => p.timeMs)).toEqual([
      1_700_000_000_000, 1_700_000_060_000, 1_700_000_120_000, 1_700_000_180_000,
    ]);
  });

  it("keeps the whole curve when a feed message arrives before the read answers", async () => {
    let finish: (outcome: Outcome<AccountView>) => void = () => undefined;
    const { store, feeds, settle, open } = harness({ account: () => new Promise((resolve) => { finish = resolve; }) });
    await open();
    store.track(ADDRESS);
    feeds[0]?.handlers.onMessage(JSON.stringify({ type: "account", account: viewBody({ state: "halted", curve: [{ time_ms: 1_700_000_180_000, equity: "7" }] }) }));
    finish(ok<AccountView>(readAccount(viewBody({ curve: FULL_CURVE }))));
    await settle();

    expect(store.getSnapshot().curves.get(ADDRESS)).toHaveLength(4);
    expect(store.getSnapshot().accounts[0]?.state).toBe("halted");
  });
});
