/** Tests of the funding store over a stand-in API and clock. */
import type { Api, Outcome } from "./client";
import { fundingBody, operationBody } from "./funding-fixtures";
import { readFunding } from "./funding-parsers";
import { BUSY_POLL_MS, FundingStore, IDLE_POLL_MS } from "./funding-store";
import type { FundingRead } from "./funding-types";

const idleRead = readFunding(fundingBody());
const busyRead = readFunding(fundingBody({ operations: [operationBody()] }));
const idle: Outcome<FundingRead> = { status: "ok", value: idleRead };
const busy: Outcome<FundingRead> = { status: "ok", value: busyRead };

/** An API whose only working call is the funding read. */
function onlyFunding(funding: Api["funding"]): Api {
  const unused = () => Promise.reject(new Error("the store makes no such call"));
  return {
    session: unused, login: unused, logout: unused, accounts: unused, account: unused, setAccountRunning: unused,
    flatten: unused, setStrategyRunning: unused, setParams: unused, declare: unused, deposit: unused, claim: unused,
    withdraw: unused, funding,
  };
}

/** The store with the test in charge of time and of each read's answer. */
function harness(answers: Outcome<FundingRead>[]) {
  const timers: { callback: () => void; ms: number; cancelled: boolean }[] = [];
  const ended: string[] = [];
  let reads = 0;
  const store = new FundingStore({
    api: onlyFunding(() => Promise.resolve(answers[Math.min(reads++, answers.length - 1)] ?? idle)),
    timeout: (callback, ms) => {
      const timer = { callback, ms, cancelled: false };
      timers.push(timer);
      return () => { timer.cancelled = true; };
    },
    onUnauthenticated: () => { ended.push("ended"); },
  });
  const settle = () => new Promise<void>((resolve) => { setTimeout(resolve, 0); });
  const live = () => timers.filter((timer) => !timer.cancelled);
  return { store, timers, live, ended, settle, reads: () => reads };
}

describe("FundingStore", () => {
  it("starts loading, then holds the first read as live", async () => {
    const { store, settle } = harness([idle]);
    expect(store.getSnapshot()).toEqual({ status: "loading", read: null, failure: null });
    store.start();
    await settle();
    expect(store.getSnapshot()).toEqual({ status: "live", read: idleRead, failure: null });
  });

  it("reads again after the idle wait, and sooner while an operation is pending", async () => {
    const { store, live, settle } = harness([idle, busy, busy]);
    store.start();
    await settle();
    expect(live().map((timer) => timer.ms)).toEqual([IDLE_POLL_MS]);
    live()[0]?.callback();
    await settle();
    expect(live().map((timer) => timer.ms)).toEqual([IDLE_POLL_MS, BUSY_POLL_MS]);
  });

  it("keeps the last read and says it is offline when a read fails, then recovers", async () => {
    const down: Outcome<FundingRead> = { status: "unavailable", message: "The bot did not answer." };
    const { store, live, settle } = harness([idle, down, idle]);
    store.start();
    await settle();
    live()[0]?.callback();
    await settle();
    const offline = store.getSnapshot();
    expect(offline.status).toBe("offline");
    expect(offline.failure).toBe("The bot did not answer.");
    expect(offline.read).not.toBeNull();
    live()[1]?.callback();
    await settle();
    expect(store.getSnapshot().status).toBe("live");
  });

  it("ends the session on a 401 and schedules nothing", async () => {
    const { store, live, ended, settle } = harness([{ status: "unauthenticated" }]);
    store.start();
    await settle();
    expect(ended).toEqual(["ended"]);
    expect(live()).toEqual([]);
  });

  it("takes an action's answer over a read that began before it", async () => {
    let release: (outcome: Outcome<FundingRead>) => void = () => undefined;
    const slow = new Promise<Outcome<FundingRead>>((resolve) => { release = resolve; });
    const store = new FundingStore({
      api: onlyFunding(() => slow),
      timeout: () => () => undefined,
      onUnauthenticated: () => undefined,
    });
    store.start();
    store.accept(busyRead);
    release(idle);
    await new Promise<void>((resolve) => { setTimeout(resolve, 0); });
    expect(store.getSnapshot().read).toBe(busyRead);
  });

  it("stops cleanly: no timer fires, and a restart begins from loading", async () => {
    const { store, live, settle, reads } = harness([idle]);
    const stop = store.start();
    await settle();
    stop();
    expect(live()).toEqual([]);
    expect(store.getSnapshot().status).toBe("loading");
    store.start();
    await settle();
    expect(reads()).toBe(2);
  });

  it("ignores a read that finishes after a stop", async () => {
    let release: (outcome: Outcome<FundingRead>) => void = () => undefined;
    const slow = new Promise<Outcome<FundingRead>>((resolve) => { release = resolve; });
    const store = new FundingStore({
      api: onlyFunding(() => slow),
      timeout: () => () => undefined,
      onUnauthenticated: () => undefined,
    });
    store.start()();
    release(idle);
    await new Promise<void>((resolve) => { setTimeout(resolve, 0); });
    expect(store.getSnapshot().status).toBe("loading");
  });
});
