/** Tests of the live store over a stand-in feed, API and clock. */
import type { Api, Outcome } from "./client";
import { detailBody, liveBody, summaryBody } from "./fixtures";
import { LiveStore, type FeedHandlers } from "./live-store";
import { readAccount, readAccountList } from "./parsers";
import type { AccountList, AccountRead } from "./types";

const ADDRESS = `0x${"11".repeat(20)}`;

/** Everything the store touches, with the test in charge of time and the feed. */
function harness(api: Partial<Api> = {}) {
  const feeds: { handlers: FeedHandlers; closed: boolean }[] = [];
  const frames: (() => void)[] = [];
  const timers: { callback: () => void; ms: number }[] = [];
  const ended: string[] = [];
  const list: Outcome<AccountList> = { status: "ok", value: readAccountList({ seq: 10, accounts: [summaryBody()] }) };
  const detail: Outcome<AccountRead> = { status: "ok", value: readAccount({ seq: 10, account: detailBody() }) };
  const unused = () => Promise.reject(new Error("the store makes no such call"));
  const store = new LiveStore({
    api: {
      session: unused, login: unused, logout: unused, setAccountRunning: unused, flatten: unused,
      setStrategyRunning: unused, setParams: unused, funding: unused, declare: unused, deposit: unused, claim: unused,
      withdraw: unused,
      accounts: () => Promise.resolve(list),
      account: () => Promise.resolve(detail),
      ...api,
    },
    openFeed: (handlers) => {
      const feed = { handlers, closed: false };
      feeds.push(feed);
      return { close: () => { feed.closed = true; } };
    },
    frame: (callback) => {
      frames.push(callback);
      return () => undefined;
    },
    timeout: (callback, ms) => {
      timers.push({ callback, ms });
      return () => undefined;
    },
    random: () => 0,
    onUnauthenticated: () => { ended.push("ended"); },
  });
  const flush = () => {
    for (const run of frames.splice(0)) {
      run();
    }
  };
  const settle = async () => { await new Promise((resolve) => { setTimeout(resolve, 0); }); flush(); };
  return { store, feeds, timers, ended, flush, settle };
}

const accountMessage = (seq: number, equity: string) =>
  JSON.stringify({ type: "account", seq, account: liveBody({ equity }) });

describe("LiveStore", () => {
  it("reads the list on open and is live only once that read has finished", async () => {
    const { store, feeds, settle, flush } = harness();
    store.start();
    flush();
    expect(store.getSnapshot().connection).toBe("connecting");

    feeds[0]?.handlers.onOpen();
    await settle();

    expect(store.getSnapshot().connection).toBe("live");
    expect(store.getSnapshot().loaded).toBe(true);
    expect(store.getSnapshot().accounts.map((a) => a.equity)).toEqual(["10100.5"]);
  });

  it("holds messages that arrive during the read and applies the newer ones after it", async () => {
    const { store, feeds, settle } = harness();
    store.start();
    feeds[0]?.handlers.onOpen();
    feeds[0]?.handlers.onMessage(accountMessage(9, "1"));
    feeds[0]?.handlers.onMessage(accountMessage(11, "2"));
    await settle();

    expect(store.getSnapshot().accounts.map((a) => a.equity)).toEqual(["2"]);
  });

  it("counts a message that fails its check and shows nothing of it", async () => {
    const { store, feeds, settle } = harness();
    store.start();
    feeds[0]?.handlers.onOpen();
    await settle();
    feeds[0]?.handlers.onMessage("not json");
    feeds[0]?.handlers.onMessage(JSON.stringify({ type: "account", seq: 12, account: liveBody({ equity: 5 }) }));
    await settle();

    expect(store.getSnapshot().dropped).toBe(2);
    expect(store.getSnapshot().accounts.map((a) => a.equity)).toEqual(["10100.5"]);
  });

  it("publishes once per frame however many messages arrive", async () => {
    const { store, feeds, settle, flush } = harness();
    store.start();
    feeds[0]?.handlers.onOpen();
    await settle();
    let published = 0;
    store.subscribe(() => { published += 1; });
    for (let seq = 11; seq < 61; seq += 1) {
      feeds[0]?.handlers.onMessage(accountMessage(seq, String(seq)));
    }
    flush();

    expect(published).toBe(1);
    expect(store.getSnapshot().accounts[0]?.equity).toBe("60");
  });

  it("reconnects after a close with a capped, jittered wait, and resyncs before going live", async () => {
    const { store, feeds, timers, settle, flush } = harness();
    store.start();
    feeds[0]?.handlers.onOpen();
    await settle();

    feeds[0]?.handlers.onClose();
    feeds[0]?.handlers.onClose();
    flush();
    expect(store.getSnapshot().connection).toBe("reconnecting");
    expect(timers).toHaveLength(1);
    expect(timers[0]?.ms).toBe(250);

    timers[0]?.callback();
    expect(feeds).toHaveLength(2);
    feeds[1]?.handlers.onOpen();
    await settle();
    expect(store.getSnapshot().connection).toBe("live");
  });

  it("ignores the late events of a connection that a stop and start replaced", async () => {
    const { store, feeds, settle } = harness();
    const stop = store.start();
    stop();
    store.start();
    feeds[0]?.handlers.onOpen();
    feeds[0]?.handlers.onMessage(accountMessage(50, "666"));
    await settle();
    expect(store.getSnapshot().connection).toBe("connecting");
    expect(feeds).toHaveLength(2);
    expect(feeds[0]?.closed).toBe(true);
  });

  it("forgets every account on stop", async () => {
    const { store, feeds, settle } = harness();
    const stop = store.start();
    feeds[0]?.handlers.onOpen();
    await settle();
    stop();
    await settle();

    expect(store.getSnapshot()).toMatchObject({ connection: "stopped", loaded: false, accounts: [] });
    expect(feeds[0]?.closed).toBe(true);
  });

  it("ends the session when the read says 401", async () => {
    const { store, feeds, ended, settle } = harness({ accounts: () => Promise.resolve({ status: "unauthenticated" }) });
    store.start();
    feeds[0]?.handlers.onOpen();
    await settle();

    expect(ended).toEqual(["ended"]);
    expect(store.getSnapshot().connection).toBe("stopped");
  });

  it("reads a tracked account's detail and keeps the reason when it cannot", async () => {
    const { store, feeds, settle } = harness({
      account: () => Promise.resolve({ status: "unavailable", message: "engine unavailable" }),
    });
    store.start();
    feeds[0]?.handlers.onOpen();
    await settle();
    store.track(ADDRESS);
    await settle();

    expect(store.getSnapshot().detailFailures.get(ADDRESS)).toBe("engine unavailable");
    expect(store.getSnapshot().details.size).toBe(0);
  });

  it("takes an account a control returned as the newest read", async () => {
    const { store, feeds, settle, flush } = harness();
    store.start();
    feeds[0]?.handlers.onOpen();
    await settle();
    const read = readAccount({ seq: 20, account: detailBody({ state: "stopped" }) });
    store.accept(read);
    flush();
    feeds[0]?.handlers.onMessage(accountMessage(15, "1"));
    flush();

    expect(store.getSnapshot().accounts[0]?.state).toBe("stopped");
    expect(store.getSnapshot().accounts[0]?.equity).toBe("10100.5");
  });
});
