/** The wait before a reconnect: doubling from a base to a cap, with jitter. */

/** The first wait, in milliseconds. */
export const BACKOFF_BASE_MS = 500;
/** The longest wait, in milliseconds. */
export const BACKOFF_CAP_MS = 15_000;

/**
 * The wait before reconnect number `attempt` (from 0): half to all of the doubled base, up to the cap.
 * `random` is a value in [0, 1) the caller supplies.
 */
export function reconnectDelay(attempt: number, random: number): number {
  const doubled = Math.min(BACKOFF_CAP_MS, BACKOFF_BASE_MS * 2 ** Math.min(attempt, 30));
  return Math.floor(doubled * (0.5 + random / 2));
}
