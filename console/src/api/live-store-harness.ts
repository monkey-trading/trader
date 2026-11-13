/** A live store over a stand-in feed, API and clock, for its tests. */
import type { Api, Outcome } from "./client";
import { figuresBody, fillBody, overviewBody, totalsBody, viewBody } from "./fixtures";
import { LiveStore } from "./live-store";
import type { FeedHandlers } from "./live-types";
import { readAccount, readFills } from "./parsers";
import type { AccountView, Fill } from "./types";

export const ok = <T>(value: T): Outcome<T> => ({ status: "ok", value });
export const snapshot = (equity = "10100.5") =>
  JSON.stringify({ type: "snapshot", ...overviewBody([viewBody({ figures: figuresBody({ equity }) })], totalsBody({ equity })) });

/** Everything the store touches, with the test in charge of time and the feed. */
export function harness(api: Partial<Api> = {}) {
  const feeds: { handlers: FeedHandlers; closed: boolean }[] = [];
  const frames: (() => void)[] = [];
  const timers: { callback: () => void; ms: number }[] = [];
  /** The waits for the feed's next message, apart from the reconnect waits. */
  const silences: { callback: () => void; ms: number }[] = [];
  const ended: string[] = [];
  const unused = () => Promise.reject(new Error("the store makes no such call"));
  const store = new LiveStore({
    api: {
      login: unused, logout: unused, setAccountRunning: unused, flatten: unused, setStrategyRunning: unused, setParams: unused,
      session: () => Promise.resolve(ok(null)),
      account: () => Promise.resolve(ok<AccountView>(readAccount(viewBody()))),
      fills: () => Promise.resolve(ok<readonly Fill[]>(readFills({ fills: [fillBody()] }))),
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
      (ms >= 60_000 ? silences : timers).push({ callback, ms });
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
  const settle = async () => {
    await new Promise((resolve) => { setTimeout(resolve, 0); });
    flush();
  };
  /** Starts the store and delivers the feed's snapshot. */
  const open = async (equity?: string) => {
    const stop = store.start();
    feeds[0]?.handlers.onOpen();
    feeds[0]?.handlers.onMessage(snapshot(equity));
    await settle();
    return stop;
  };
  return { store, feeds, timers, silences, ended, flush, settle, open };
}

export const accountMessage = (equity: string) =>
  JSON.stringify({ type: "account", account: viewBody({ figures: figuresBody({ equity }) }) });

