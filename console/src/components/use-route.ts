/** The current path, kept in step with the browser's history. */
import { useSyncExternalStore } from "react";

import { parseRoute, type Route } from "@/lib/route";

/** Subscribes to history changes: back and forward, and the console's own navigations. */
function subscribe(listener: () => void): () => void {
  window.addEventListener("popstate", listener);
  return () => {
    window.removeEventListener("popstate", listener);
  };
}

/** The page the URL names. */
export function useRoute(): Route {
  const pathname = useSyncExternalStore(subscribe, () => window.location.pathname);
  return parseRoute(pathname);
}

/** Moves to `path` without a page load and tells every `useRoute` to re-read. */
export function navigate(path: string): void {
  window.history.pushState(null, "", path);
  window.dispatchEvent(new PopStateEvent("popstate"));
}
