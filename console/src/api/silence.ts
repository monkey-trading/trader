/** A wait for a message that should come, started over at each one. */
import type { LiveDeps } from "./live-types";

/** How long the live feed may say nothing, while accounts report, before it is taken as lost. */
export const FEED_SILENCE_MS = 60_000;

/** Calls back when `ms` pass without a restart. */
export class Silence {
  private cancel: (() => void) | null = null;
  private readonly timeout: LiveDeps["timeout"];

  constructor(timeout: LiveDeps["timeout"]) {
    this.timeout = timeout;
  }

  /** Starts the wait over; `onSilent` runs if it ends. */
  restart(ms: number, onSilent: () => void): void {
    this.clear();
    this.cancel = this.timeout(onSilent, ms);
  }

  /** Ends the wait, with no call. */
  clear(): void {
    this.cancel?.();
    this.cancel = null;
  }
}
