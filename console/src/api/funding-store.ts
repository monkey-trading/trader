/** The funding store: the accounts' wallets, declarations and operations, read again while the page is open. */
import type { Api } from "./client";
import type { FundingRead } from "./funding-types";

/** Whether the figures shown are the bot's latest answer. */
export type FundingStatus = "loading" | "live" | "offline";

/** What components read. */
export interface FundingSnapshot {
  readonly status: FundingStatus;
  /** The latest answer; kept while offline so the page does not blank, and marked out of date. */
  readonly read: FundingRead | null;
  /** Why the last read failed; null while live. */
  readonly failure: string | null;
}

/** What the store needs from the outside world. */
export interface FundingDeps {
  readonly api: Api;
  /** Runs `callback` after `ms`; returns a canceller. */
  readonly timeout: (callback: () => void, ms: number) => () => void;
  /** Called when the bot says the session is gone. */
  readonly onUnauthenticated: () => void;
}

/** Milliseconds between reads while an operation is pending, and otherwise. */
export const BUSY_POLL_MS = 1_000;
export const IDLE_POLL_MS = 5_000;

const INITIAL: FundingSnapshot = { status: "loading", read: null, failure: null };

/** Holds the latest funding read and keeps it current by reading again on a timer. */
export class FundingStore {
  private readonly deps: FundingDeps;
  private snapshot: FundingSnapshot = INITIAL;
  private readonly listeners = new Set<() => void>();
  /** Bumped on every start and stop, so a late answer for an old run is ignored. */
  private epoch = 0;
  /** Bumped when an action's answer is taken, so a read begun before it cannot overwrite it. */
  private generation = 0;
  private cancelNext: (() => void) | null = null;

  constructor(deps: FundingDeps) {
    this.deps = deps;
  }

  /** Subscribes `listener` to published snapshots; returns the unsubscriber. */
  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  /** The latest snapshot. */
  readonly getSnapshot = (): FundingSnapshot => this.snapshot;

  /** Starts reading. Returns what undoes it fully, so a repeated start after a stop begins clean. */
  start(): () => void {
    this.epoch += 1;
    void this.poll(this.epoch);
    return () => {
      this.epoch += 1;
      this.cancelNext?.();
      this.cancelNext = null;
      this.snapshot = INITIAL;
      this.notify();
    };
  }

  /** Takes the answer of a funding action as the freshest read, and reads sooner while it is pending. */
  accept(read: FundingRead): void {
    this.generation += 1;
    this.snapshot = { status: "live", read, failure: null };
    this.notify();
    this.cancelNext?.();
    this.schedule(this.epoch, read);
  }

  private async poll(epoch: number): Promise<void> {
    const generation = this.generation;
    const outcome = await this.deps.api.funding();
    if (epoch !== this.epoch) {
      return;
    }
    switch (outcome.status) {
      case "ok":
        if (generation === this.generation) {
          this.snapshot = { status: "live", read: outcome.value, failure: null };
          this.notify();
          this.schedule(epoch, outcome.value);
        }
        return;
      case "unauthenticated":
        this.deps.onUnauthenticated();
        return;
      case "refused":
      case "unavailable":
        this.snapshot = { status: "offline", read: this.snapshot.read, failure: outcome.message };
        this.notify();
        this.schedule(epoch, null);
        return;
    }
  }

  private schedule(epoch: number, read: FundingRead | null): void {
    const busy = read?.operations.some((operation) => operation.state === "pending") ?? false;
    this.cancelNext = this.deps.timeout(() => {
      if (epoch === this.epoch) {
        void this.poll(epoch);
      }
    }, busy ? BUSY_POLL_MS : IDLE_POLL_MS);
  }

  private notify(): void {
    for (const listener of this.listeners) {
      listener();
    }
  }
}
