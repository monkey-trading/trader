/** The live store's public types: what components read, and what the store needs from outside. */
import type { Api } from "./client";
import type { AccountView, CurvePoint, Fill, Totals } from "./types";

/** Where the connection stands; nothing is shown as live unless it is `live`. */
export type Connection = "connecting" | "syncing" | "live" | "reconnecting" | "stopped";

/** What components read. A new object is published at most once per animation frame. */
export interface LiveSnapshot {
  readonly connection: Connection;
  /** Feed messages that failed their check since the page loaded. */
  readonly dropped: number;
  /** False until the feed's first snapshot has arrived. */
  readonly loaded: boolean;
  /** Every account, in the order the bot lists them. */
  readonly accounts: readonly AccountView[];
  readonly totals: Totals | null;
  /** Each account's equity curve by lower-case address. */
  readonly curves: ReadonlyMap<string, readonly CurvePoint[]>;
  readonly fills: ReadonlyMap<string, readonly Fill[]>;
  /** Why an account's fills could not be read, by lower-case address. */
  readonly fillFailures: ReadonlyMap<string, string>;
  /** Why an account's full equity curve could not be read, by lower-case address. */
  readonly curveFailures: ReadonlyMap<string, string>;
}

/** One open feed connection. */
export interface FeedSocket {
  close(): void;
}

/** What the store hands the feed to report. */
export interface FeedHandlers {
  readonly onOpen: () => void;
  readonly onMessage: (data: string) => void;
  readonly onClose: () => void;
}

/** What the store needs from the outside world. */
export interface LiveDeps {
  readonly api: Api;
  readonly openFeed: (handlers: FeedHandlers) => FeedSocket;
  /** Runs `callback` on the next animation frame; returns a canceller. */
  readonly frame: (callback: () => void) => () => void;
  /** Runs `callback` after `ms`; returns a canceller. */
  readonly timeout: (callback: () => void, ms: number) => () => void;
  readonly random: () => number;
  /** Called when the bot says the session is gone. */
  readonly onUnauthenticated: () => void;
}

/** What a store that has not started shows. */
export const INITIAL_SNAPSHOT: LiveSnapshot = {
  connection: "stopped",
  dropped: 0,
  loaded: false,
  accounts: [],
  totals: null,
  curves: new Map(),
  fills: new Map(),
  fillFailures: new Map(),
  curveFailures: new Map(),
};
