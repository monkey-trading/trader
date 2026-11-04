/** Renders the app over stand-in API calls and a feed the test drives. */
import { render } from "@testing-library/react";
import { vi } from "vitest";

import type { Api, Outcome } from "@/api/client";
import { detailBody, summaryBody } from "@/api/fixtures";
import { fundingBody } from "@/api/funding-fixtures";
import { readFunding } from "@/api/funding-parsers";
import { FundingStore } from "@/api/funding-store";
import { LiveStore, type FeedHandlers } from "@/api/live-store";
import { readAccount, readAccountList } from "@/api/parsers";
import { SessionStore } from "@/api/session-store";
import type { FundingRead } from "@/api/funding-types";
import type { AccountRead } from "@/api/types";
import { App } from "@/App";
import { AppContext } from "@/components/app-context";

export const ADDRESS = `0x${"11".repeat(20)}`;

/** The calls the app made, as mocks to assert on and to give different answers. */
export function stubApi(): { [K in keyof Api]: ReturnType<typeof vi.fn<Api[K]>> } {
  const read = (overrides: Record<string, unknown> = {}): Outcome<AccountRead> => ({
    status: "ok",
    value: readAccount({ seq: 10, account: detailBody(overrides) }),
  });
  const funded = (): Outcome<FundingRead> => ({ status: "ok", value: readFunding(fundingBody()) });
  const nothing: Outcome<null> = { status: "ok", value: null };
  return {
    session: vi.fn<Api["session"]>(() => Promise.resolve(nothing)),
    login: vi.fn<Api["login"]>(() => Promise.resolve(nothing)),
    logout: vi.fn<Api["logout"]>(() => Promise.resolve(nothing)),
    accounts: vi.fn<Api["accounts"]>(() =>
      Promise.resolve({ status: "ok", value: readAccountList({ seq: 10, accounts: [summaryBody()] }) }),
    ),
    account: vi.fn<Api["account"]>(() => Promise.resolve(read())),
    setAccountRunning: vi.fn<Api["setAccountRunning"]>(() => Promise.resolve(read({ state: "stopped" }))),
    flatten: vi.fn<Api["flatten"]>(() => Promise.resolve(read({ positions: [], openPositions: 0 }))),
    setStrategyRunning: vi.fn<Api["setStrategyRunning"]>(() => Promise.resolve(read())),
    setParams: vi.fn<Api["setParams"]>(() => Promise.resolve(read())),
    funding: vi.fn<Api["funding"]>(() => Promise.resolve(funded())),
    declare: vi.fn<Api["declare"]>(() => Promise.resolve(funded())),
    deposit: vi.fn<Api["deposit"]>(() => Promise.resolve(funded())),
    claim: vi.fn<Api["claim"]>(() => Promise.resolve(funded())),
    withdraw: vi.fn<Api["withdraw"]>(() => Promise.resolve(funded())),
  };
}

/** Mounts `App` at `path`; frames and timers run at once, and the test drives the feed. */
export function mount(api: ReturnType<typeof stubApi>, path = "/") {
  window.history.replaceState(null, "", path);
  const feeds: FeedHandlers[] = [];
  const session = new SessionStore(api);
  const live = new LiveStore({
    api,
    openFeed: (handlers) => {
      feeds.push(handlers);
      // The browser opens a socket asynchronously; the stand-in does too.
      queueMicrotask(handlers.onOpen);
      return { close: () => undefined };
    },
    frame: (callback) => {
      const id = window.setTimeout(callback, 0);
      return () => { window.clearTimeout(id); };
    },
    timeout: (callback, ms) => {
      const id = window.setTimeout(callback, ms);
      return () => { window.clearTimeout(id); };
    },
    random: () => 0,
    onUnauthenticated: () => { session.expire(); },
  });
  const funding = new FundingStore({
    api,
    timeout: (callback, ms) => {
      const id = window.setTimeout(callback, ms);
      return () => { window.clearTimeout(id); };
    },
    onUnauthenticated: () => { session.expire(); },
  });
  const view = render(
    <AppContext value={{ api, funding, live, session }}>
      <App />
    </AppContext>,
  );
  return { ...view, feeds };
}
