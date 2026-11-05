/** The connection state in the page frame; no figure is presented as live while the feed is down. */
import type { Connection } from "@/api/live-types";

import { useLive } from "./app-context";

/** What each connection state tells the user. */
const WORDS: Readonly<Record<Connection, string>> = {
  connecting: "Connecting…",
  syncing: "Updating…",
  live: "Live",
  reconnecting: "Disconnected. Reconnecting; figures shown may be out of date.",
  stopped: "Not connected",
};

/** One status line: the connection, and how many feed messages were ignored as unreadable. */
export function ConnectionStatus() {
  const { connection, dropped } = useLive();
  return (
    <p role="status" className="text-sm text-muted">
      {WORDS[connection]}
      {dropped > 0 ? ` ${String(dropped)} unreadable feed messages ignored.` : ""}
    </p>
  );
}
