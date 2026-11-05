/** The live store: one feed connection per tab, opening with the bot's snapshot, read by components. */
import { reconnectDelay } from "@/lib/backoff";

import type { Api, Outcome } from "./client";
import { EMPTY_BOOK, keyOf, withAccount, withFills, withMessage, type Book } from "./live-state";
import { parseWith, readLiveMessage } from "./parsers";
import type { AccountView, Fill, Totals } from "./types";

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
  readonly fills: ReadonlyMap<string, readonly Fill[]>;
  /** Why an account's fills could not be read, by lower-case address. */
  readonly fillFailures: ReadonlyMap<string, string>;
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

const INITIAL: LiveSnapshot = {
  connection: "stopped",
  dropped: 0,
  loaded: false,
  accounts: [],
  totals: null,
  fills: new Map(),
  fillFailures: new Map(),
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
  /** Per account, how many feed messages have set its view or its fills; a read that began before one is stale. */
  private readonly viewVersions = new Map<string, number>();
  private readonly fillVersions = new Map<string, number>();
  private socket: FeedSocket | null = null;
  private attempt = 0;
  /** Bumped on every connection start, stop and failure, so a late event of an old one is ignored. */
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

  /** Asks for an account's fills to be read and kept current while the caller shows them; returns the release. */
  track(address: string): () => void {
    this.tracked.set(address, (this.tracked.get(address) ?? 0) + 1);
    void this.readFills(address, this.epoch);
    return () => {
      const left = (this.tracked.get(address) ?? 1) - 1;
      if (left <= 0) {
        this.tracked.delete(address);
      } else {
        this.tracked.set(address, left);
      }
    };
  }

  /** Reads an account again, after a control changed it, so the page shows the result before the feed's next push. */
  async refresh(address: string): Promise<void> {
    const epoch = this.epoch;
    const key = keyOf(address);
    const version = this.viewVersions.get(key) ?? 0;
    const outcome = await this.deps.api.account(address);
    if (epoch !== this.epoch || (this.viewVersions.get(key) ?? 0) !== version) {
      return;
    }
    if (outcome.status === "ok") {
      this.book = withAccount(this.book, outcome.value);
      this.publish();
    } else if (outcome.status === "unauthenticated") {
      this.endSession();
    }
  }

  private stop(): void {
    this.epoch += 1;
    this.cancelRetry?.();
    this.cancelRetry = null;
    this.socket?.close();
    this.socket = null;
    this.book = EMPTY_BOOK;
    this.loaded = false;
    this.failures = new Map();
    this.viewVersions.clear();
    this.fillVersions.clear();
    this.connection = "stopped";
    this.publish();
  }

  private endSession(): void {
    this.stop();
    this.deps.onUnauthenticated();
  }

  private connect(state: "connecting" | "reconnecting"): void {
    const epoch = this.epoch;
    this.connection = state;
    this.publish();
    this.socket = this.deps.openFeed({
      onOpen: () => {
        if (epoch === this.epoch) {
          this.connection = "syncing";
          this.publish();
        }
      },
      onMessage: (data) => {
        if (epoch === this.epoch) {
          this.receive(data, epoch);
        }
      },
      onClose: () => {
        if (epoch === this.epoch) {
          void this.lostFeed();
        }
      },
    });
  }

  /** The feed ended: ask whether the session is still good, then retry after a capped, jittered wait. */
  private async lostFeed(): Promise<void> {
    // The old socket's late close event must not start a second retry.
    this.epoch += 1;
    const epoch = this.epoch;
    this.socket?.close();
    this.socket = null;
    this.connection = "reconnecting";
    this.publish();
    const session: Outcome<null> = await this.deps.api.session();
    if (epoch !== this.epoch) {
      return;
    }
    if (session.status === "unauthenticated") {
      this.endSession();
      return;
    }
    const wait = reconnectDelay(this.attempt, this.deps.random());
    this.attempt += 1;
    this.cancelRetry = this.deps.timeout(() => {
      if (epoch === this.epoch) {
        this.connect("reconnecting");
      }
    }, wait);
  }

  private receive(data: string, epoch: number): void {
    let json: unknown;
    try {
      json = JSON.parse(data);
    } catch {
      this.drop();
      return;
    }
    const parsed = parseWith(readLiveMessage, json);
    // The bot opens every feed with its snapshot; anything before it is not part of the protocol.
    if (parsed.status === "invalid" || (!this.loadedOnThisFeed() && parsed.value.type !== "snapshot")) {
      this.drop();
      return;
    }
    const message = parsed.value;
    if (message.type === "account") {
      this.bump(this.viewVersions, message.account.address);
    } else if (message.type === "fills") {
      this.bump(this.fillVersions, message.address);
    }
    this.book = withMessage(this.book, message);
    if (message.type === "snapshot") {
      this.loaded = true;
      this.attempt = 0;
      this.connection = "live";
      for (const account of message.overview.accounts) {
        this.bump(this.viewVersions, account.address);
      }
      for (const address of this.tracked.keys()) {
        void this.readFills(address, epoch);
      }
    }
    this.publish();
  }

  /** Whether the open connection has delivered its snapshot. */
  private loadedOnThisFeed(): boolean {
    return this.connection === "live";
  }

  private drop(): void {
    this.dropped += 1;
    this.publish();
  }

  private bump(versions: Map<string, number>, address: string): void {
    const key = keyOf(address);
    versions.set(key, (versions.get(key) ?? 0) + 1);
  }

  /** Reads an account's fills; a feed message that arrived meanwhile is newer than the read. */
  private async readFills(address: string, epoch: number): Promise<void> {
    const key = keyOf(address);
    const version = this.fillVersions.get(key) ?? 0;
    const outcome = await this.deps.api.fills(address);
    if (epoch !== this.epoch || (this.fillVersions.get(key) ?? 0) !== version) {
      return;
    }
    if (outcome.status === "ok") {
      this.book = withFills(this.book, address, outcome.value);
      this.failures = new Map([...this.failures].filter(([failed]) => failed !== key));
    } else if (outcome.status === "unauthenticated") {
      this.endSession();
      return;
    } else {
      this.failures = new Map(this.failures).set(key, outcome.message);
    }
    this.publish();
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
        accounts: [...this.book.accounts.values()],
        totals: this.book.totals,
        fills: this.book.fills,
        fillFailures: this.failures,
      };
      for (const listener of this.listeners) {
        listener();
      }
    });
  }
}
