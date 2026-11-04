/** The stores and API client the app shares, created once in `main.tsx`. */
import { createContext, use, useSyncExternalStore } from "react";

import type { Api } from "@/api/client";
import type { LiveSnapshot, LiveStore } from "@/api/live-store";
import type { SessionStatus, SessionStore } from "@/api/session-store";

/** Everything a component may need from outside React. */
export interface AppServices {
  readonly api: Api;
  readonly live: LiveStore;
  readonly session: SessionStore;
}

/** Carries the services; there is no default, so a missing provider fails loudly. */
export const AppContext = createContext<AppServices | null>(null);

/** The app's services. */
export function useServices(): AppServices {
  const services = use(AppContext);
  if (services === null) {
    // Every render happens under the provider in main.tsx or a test's wrapper.
    throw new Error("no AppContext provider above this component");
  }
  return services;
}

/** The live book, re-rendering the caller only when a new snapshot is published. */
export function useLive(): LiveSnapshot {
  const { live } = useServices();
  return useSyncExternalStore(live.subscribe, live.getSnapshot);
}

/** The session status. */
export function useSession(): SessionStatus {
  const { session } = useServices();
  return useSyncExternalStore(session.subscribe, session.getSnapshot);
}
