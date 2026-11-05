/** Renders the app over stand-in API calls and a feed the test drives. */
import { render } from "@testing-library/react";
import { vi } from "vitest";

import type { Api, Outcome } from "@/api/client";
import { ADDRESS, fillBody, overviewBody, viewBody } from "@/api/fixtures";
import { LiveStore, type FeedHandlers } from "@/api/live-store";
import { readAccount, readFills } from "@/api/parsers";
import { SessionStore } from "@/api/session-store";
import { App } from "@/App";
import { AppContext } from "@/components/app-context";

export { ADDRESS };

/** The calls the app made, as mocks to assert on and to give different answers. */
export function stubApi(): { [K in keyof Api]: ReturnType<typeof vi.fn<Api[K]>> } {
  const nothing: Outcome<null> = { status: "ok", value: null };
  const done = () => Promise.resolve(nothing);
  return {
    session: vi.fn<Api["session"]>(done),
    login: vi.fn<Api["login"]>(done),
    logout: vi.fn<Api["logout"]>(done),
    account: vi.fn<Api["account"]>(() => Promise.resolve({ status: "ok", value: readAccount(viewBody()) })),
    fills: vi.fn<Api["fills"]>(() => Promise.resolve({ status: "ok", value: readFills({ fills: [fillBody()] }) })),
    setAccountRunning: vi.fn<Api["setAccountRunning"]>(done),
    flatten: vi.fn<Api["flatten"]>(done),
    setStrategyRunning: vi.fn<Api["setStrategyRunning"]>(done),
    setParams: vi.fn<Api["setParams"]>(done),
  };
}

/**
 * Mounts `App` at `path`; frames and timers run at once, and the feed opens with a snapshot of `overview`.
 * The test drives later messages through `feeds`.
 */
export function mount(api: ReturnType<typeof stubApi>, path = "/", overview: Record<string, unknown> = overviewBody()) {
  window.history.replaceState(null, "", path);
  const feeds: FeedHandlers[] = [];
  const session = new SessionStore(api);
  const live = new LiveStore({
    api,
    openFeed: (handlers) => {
      feeds.push(handlers);
      // The browser opens a socket asynchronously, and the bot sends its snapshot first.
      queueMicrotask(() => {
        handlers.onOpen();
        handlers.onMessage(JSON.stringify({ type: "snapshot", ...overview }));
      });
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
  const view = render(
    <AppContext value={{ api, live, session }}>
      <App />
    </AppContext>,
  );
  return { ...view, feeds };
}
