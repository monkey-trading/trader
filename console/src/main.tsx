/** Mounts the console into the page. */
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { createApi } from "@/api/client";
import { openBrowserFeed } from "@/api/feed";
import { LiveStore } from "@/api/live-store";
import { SessionStore } from "@/api/session-store";
import { App } from "@/App";
import { AppContext } from "@/components/app-context";

import "@/styles.css";

const root = document.getElementById("root");
if (root === null) {
  // index.html always carries the mount point; its absence is a build defect.
  throw new Error("index.html has no #root element");
}

const api = createApi((input, init) => fetch(input, init));
const session = new SessionStore(api);
const live = new LiveStore({
  api,
  openFeed: openBrowserFeed,
  frame: (callback) => {
    const id = requestAnimationFrame(callback);
    return () => {
      cancelAnimationFrame(id);
    };
  },
  timeout: (callback, ms) => {
    const id = window.setTimeout(callback, ms);
    return () => {
      window.clearTimeout(id);
    };
  },
  random: () => Math.random(),
  onUnauthenticated: () => {
    session.expire();
  },
});

createRoot(root).render(
  <StrictMode>
    <AppContext value={{ api, live, session }}>
      <App />
    </AppContext>
  </StrictMode>,
);
