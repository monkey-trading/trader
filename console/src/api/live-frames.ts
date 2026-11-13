/** The pure steps between the live store's book and the feed: reading a frame, and publishing the book. */
import type { Book } from "./live-state";
import type { Connection, LiveSnapshot } from "./live-types";
import { parseWith, readLiveMessage } from "./parsers";
import type { ReadFailures } from "./read-failures";
import type { LiveMessage } from "./types";

/** The message in a feed frame, or null when it is not JSON, fails its check, or comes before the snapshot. */
export function readFrame(data: string, snapshotSeen: boolean): LiveMessage | null {
  let json: unknown;
  try {
    json = JSON.parse(data);
  } catch {
    return null;
  }
  const parsed = parseWith(readLiveMessage, json);
  return parsed.status === "invalid" || (!snapshotSeen && parsed.value.type !== "snapshot") ? null : parsed.value;
}

/** What components read of the store's state. */
export function snapshotOf(
  connection: Connection,
  dropped: number,
  loaded: boolean,
  book: Book,
  failures: ReadFailures,
): LiveSnapshot {
  return {
    connection,
    dropped,
    loaded,
    accounts: [...book.accounts.values()],
    totals: book.totals,
    curves: book.curves,
    fills: book.fills,
    fillFailures: failures.fills,
    curveFailures: failures.curves,
  };
}
