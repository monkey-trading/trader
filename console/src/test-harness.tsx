/** Renders the app over stand-in API calls and a feed the test drives. */
import { render } from "@testing-library/react";
import { vi } from "vitest";

import type { Api, Outcome } from "@/api/client";
import { ADDRESS, FULL_CURVE, fillBody, overviewBody, viewBody } from "@/api/fixtures";
import { LiveStore } from "@/api/live-store";
import type { FeedHandlers } from "@/api/live-types";
import { readAccount, readFills, readOverview } from "@/api/parsers";
import { SessionStore } from "@/api/session-store";
import { App } from "@/App";
import { AppContext } from "@/components/app-context";

export { ADDRESS };

/** The calls the app made, as mocks to assert on and to give different answers. */
export function stubApi(
  overview: Record<string, unknown> = overviewBody(),
): { [K in keyof Api]: ReturnType<typeof vi.fn<Api[K]>> } & { readonly overview: Record<string, unknown> } {
  const nothing: Outcome<null> = { status: "ok", value: null };
  const views = readOverview(overview).accounts;
  const done = () => Promise.resolve(nothing);
  return {
    session: vi.fn<Api["session"]>(done),
    login: vi.fn<Api["login"]>(done),
    logout: vi.fn<Api["logout"]>(done),
    // An account read is the overview's account with the whole curve, as the bot serves it.
    account: vi.fn<Api["account"]>((address) => {
      const found = views.find((view) => view.address?.toLowerCase() === address.toLowerCase());
      return Promise.resolve(
        found === undefined
          ? { status: "refused", message: "no such account", retryAfterSeconds: null }
          : { status: "ok", value: { ...found, curve: readAccount(viewBody({ curve: FULL_CURVE })).curve } },
      );
    }),
    fills: vi.fn<Api["fills"]>(() => Promise.resolve({ status: "ok", value: readFills({ fills: [fillBody()] }) })),
    setAccountRunning: vi.fn<Api["setAccountRunning"]>(done),
    flatten: vi.fn<Api["flatten"]>(done),
    setStrategyRunning: vi.fn<Api["setStrategyRunning"]>(done),
    setParams: vi.fn<Api["setParams"]>(done),
    overview,
  };
}

/**
 * Mounts `App` at `path`; frames and timers run at once, and the feed opens with a snapshot of the api's overview.
 * The test drives later messages through `feeds`.
 */
export function mount(api: ReturnType<typeof stubApi>, path = "/") {
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
        handlers.onMessage(JSON.stringify({ type: "snapshot", ...api.overview }));
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
