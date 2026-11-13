/** The wait for a message that should come, started over at each one, and judged by the pace the bot keeps. */
import type { LiveDeps } from "./live-types";

/** The least the live feed may say nothing, while accounts report, before it is taken as lost. */
export const FEED_SILENCE_MS = 60_000;
/** The most it is allowed to, whatever the pace the bot has kept. */
const MAX_SILENCE_MS = 15 * 60_000;
/** How many of the longest gaps between an account's pushes the feed may stay quiet for. */
const GAPS = 4;

/** What the store knows when a message arrives. */
interface Arming {
  /** The time between this account push's figures and the last: the bot's pace, when known. */
  readonly gapMs: number;
  /** Whether a silence matters: a feed that has not given its snapshot, or accounts that report. */
  readonly wanted: boolean;
}

/** Calls back when the feed has been quiet too long: longer than a minute, and than a few of the bot's own gaps. */
export class Silence {
  private cancel: (() => void) | null = null;
  private longestGapMs = 0;
  private readonly timeout: LiveDeps["timeout"];

  constructor(timeout: LiveDeps["timeout"]) {
    this.timeout = timeout;
  }

  /** Starts the wait over, or ends it when it does not matter; `onSilent` runs if it ends. */
  arm({ gapMs, wanted }: Arming, onSilent: () => void): void {
    this.longestGapMs = Math.max(this.longestGapMs, gapMs);
    this.clear();
    if (wanted) {
      this.cancel = this.timeout(onSilent, Math.min(MAX_SILENCE_MS, Math.max(FEED_SILENCE_MS, GAPS * this.longestGapMs)));
    }
  }

  /** Ends the wait, with no call. */
  clear(): void {
    this.cancel?.();
    this.cancel = null;
  }
}
