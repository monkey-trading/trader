/** Opens the live feed as a browser WebSocket on the page's own origin. */
import { routes } from "./client";
import type { FeedHandlers, FeedSocket } from "./live-types";

/** Opens the feed; any error ends in `onClose`, where the store decides when to retry. */
export function openBrowserFeed(handlers: FeedHandlers): FeedSocket {
  const scheme = window.location.protocol === "https:" ? "wss:" : "ws:";
  const socket = new WebSocket(`${scheme}//${window.location.host}${routes.feed}`);
  socket.addEventListener("open", handlers.onOpen);
  socket.addEventListener("message", (event: MessageEvent<unknown>) => {
    // A binary frame is not part of the protocol; the empty string fails the store's JSON check and is counted.
    handlers.onMessage(typeof event.data === "string" ? event.data : "");
  });
  socket.addEventListener("close", handlers.onClose);
  socket.addEventListener("error", handlers.onClose);
  return { close: () => { socket.close(); } };
}
