/** The live store: one feed connection per tab, resynced from the REST reads, read by components. */
import { reconnectDelay } from "@/lib/backoff";

import type { Api, Outcome } from "./client";
import { EMPTY_BOOK, withList, withMessage, withRead, type Book } from "./live-state";
import { parseWith, readLiveMessage } from "./parsers";
import type { AccountDetail, AccountRead, AccountSummary, LiveMessage } from "./types";

/** Where the connection stands; nothing is shown as live unless it is `live`. */
export type Connection = "connecting" | "syncing" | "live" | "reconnecting" | "stopped";

/** What components read. A new object is published at most once per animation frame. */
export interface LiveSnapshot {
  readonly connection: Connection;
  /** Feed messages that failed their check since the page loaded. */
  readonly dropped: number;
  /** False until the first read of the list has finished. */
  readonly loaded: boolean;
  /** Every account, ordered by name. */
  readonly accounts: readonly AccountSummary[];
  readonly details: ReadonlyMap<string, AccountDetail>;
  /** Why an account's detail could not be read, by address. */
  readonly detailFailures: ReadonlyMap<string, string>;
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

/** Messages held while the reads are in flight; past this the connection restarts. */
const BUFFER_CAP = 1000;

const INITIAL: LiveSnapshot = {
  connection: "stopped",
  dropped: 0,
  loaded: false,
  accounts: [],
  details: new Map(),
  detailFailures: new Map(),
};

/** Holds the book of accounts and keeps it current from the feed. */
export class LiveStore {
  private readonly deps: LiveDeps;
  private snapshot: LiveSnapshot = INITIAL;
  private book: Book = EMPTY_BOOK;
  private connection: Connection = "stopped";
  private dropped = 0;
  private loaded = false;
  private failures: ReadonlyMap<string, string> = new Map();
  private readonly listeners = new Set<() => void>();
  private readonly tracked = new Map<string, number>();
  private socket: FeedSocket | null = null;
  private buffer: LiveMessage[] = [];
  private attempt = 0;
  /** Bumped on every connection start and stop, so a late answer for an old one is ignored. */
  private epoch = 0;
  private cancelRetry: (() => void) | null = null;
  private cancelFrame: (() => void) | null = null;

  constructor(deps: LiveDeps) {
    this.deps = deps;
  }

  /** Subscribes `listener` to published snapshots; returns the unsubscriber. */
  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  /** The latest published snapshot. */
  readonly getSnapshot = (): LiveSnapshot => this.snapshot;

  /** Opens the feed. Returns what undoes it fully, so a repeated start after a stop begins clean. */
  start(): () => void {
    this.epoch += 1;
    this.attempt = 0;
    this.connect("connecting");
    return () => {
      this.stop();
    };
  }

  /** Asks for an account's detail to be kept current while the caller shows it; returns the release. */
  track(address: string): () => void {
    this.tracked.set(address, (this.tracked.get(address) ?? 0) + 1);
    if (this.connection === "live") {
      void this.readDetail(address, this.epoch);
    }
    return () => {
      const left = (this.tracked.get(address) ?? 1) - 1;
      if (left <= 0) {
        this.tracked.delete(address);
      } else {
        this.tracked.set(address, left);
      }
    };
  }

  /** Takes an account a control returned, as the freshest read of it. */
  accept(read: AccountRead): void {
    this.book = withRead(this.book, read);
    this.publish();
  }

  private stop(): void {
    this.epoch += 1;
    this.cancelRetry?.();
    this.cancelRetry = null;
    this.socket?.close();
    this.socket = null;
    this.buffer = [];
    this.book = EMPTY_BOOK;
    this.loaded = false;
    this.failures = new Map();
    this.connection = "stopped";
    this.publish();
  }

  private connect(state: "connecting" | "reconnecting"): void {
    const epoch = this.epoch;
    this.connection = state;
    this.buffer = [];
    this.publish();
    this.socket = this.deps.openFeed({
      onOpen: () => {
        if (epoch === this.epoch) {
          void this.sync(epoch);
        }
      },
      onMessage: (data) => {
        if (epoch === this.epoch) {
          this.receive(data);
        }
      },
      onClose: () => {
        if (epoch === this.epoch) {
          this.reconnectLater();
        }
      },
    });
  }

  private reconnectLater(): void {
    // The old socket's late close event must not schedule a second retry.
    this.epoch += 1;
    this.socket?.close();
    this.socket = null;
    this.connection = "reconnecting";
    this.publish();
    const epoch = this.epoch;
    const wait = reconnectDelay(this.attempt, this.deps.random());
    this.attempt += 1;
    this.cancelRetry = this.deps.timeout(() => {
      if (epoch === this.epoch) {
        this.connect("reconnecting");
      }
    }, wait);
  }

  /** Reads the list and every tracked detail, then applies the messages held meanwhile. */
  private async sync(epoch: number): Promise<void> {
    this.connection = "syncing";
    this.publish();
    const list = await this.deps.api.accounts();
    if (epoch !== this.epoch) {
      return;
    }
    if (!this.accepted(list)) {
      return;
    }
    this.book = withList(this.book, list.value);
    this.loaded = true;
    await Promise.all([...this.tracked.keys()].map((address) => this.readDetail(address, epoch)));
    if (epoch !== this.epoch) {
      return;
    }
    for (const message of this.buffer) {
      this.book = withMessage(this.book, message);
    }
    this.buffer = [];
    this.attempt = 0;
    this.connection = "live";
    this.publish();
  }

  /** Whether an outcome is usable; an unusable one ends the connection and schedules the next. */
  private accepted<T>(outcome: Outcome<T>): outcome is Extract<Outcome<T>, { status: "ok" }> {
    if (outcome.status === "ok") {
      return true;
    }
    if (outcome.status === "unauthenticated") {
      this.stop();
      this.deps.onUnauthenticated();
    } else {
      this.reconnectLater();
    }
    return false;
  }

  private async readDetail(address: string, epoch: number): Promise<void> {
    const outcome = await this.deps.api.account(address);
    if (epoch !== this.epoch) {
      return;
    }
    if (outcome.status === "ok") {
      this.book = withRead(this.book, outcome.value);
      this.failures = new Map([...this.failures].filter(([key]) => key !== address));
    } else if (outcome.status === "unauthenticated") {
      this.stop();
      this.deps.onUnauthenticated();
      return;
    } else {
      this.failures = new Map(this.failures).set(address, outcome.message);
    }
    this.publish();
  }

  private receive(data: string): void {
    let json: unknown;
    try {
      json = JSON.parse(data);
    } catch {
      this.dropped += 1;
      this.publish();
      return;
    }
    const parsed = parseWith(readLiveMessage, json);
    if (parsed.status === "invalid") {
      this.dropped += 1;
      this.publish();
    } else if (this.connection === "syncing") {
      if (this.buffer.length >= BUFFER_CAP) {
        this.reconnectLater();
        return;
      }
      this.buffer.push(parsed.value);
    } else if (this.connection === "live") {
      this.book = withMessage(this.book, parsed.value);
      this.publish();
    }
  }

  /** Schedules a snapshot for the next frame; any number of changes before it share one render. */
  private publish(): void {
    if (this.cancelFrame !== null) {
      return;
    }
    this.cancelFrame = this.deps.frame(() => {
      this.cancelFrame = null;
      this.snapshot = {
        connection: this.connection,
        dropped: this.dropped,
        loaded: this.loaded,
        accounts: [...this.book.summaries.values()].sort(
          (a, b) => a.name.localeCompare(b.name, "en") || a.address.localeCompare(b.address, "en"),
        ),
        details: this.book.details,
        detailFailures: this.failures,
      };
      for (const listener of this.listeners) {
        listener();
      }
    });
  }
}
