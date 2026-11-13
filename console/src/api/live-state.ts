/** The console's book of accounts and the pure steps that change it: the feed's messages and the REST reads. */
import type { AccountView, CurvePoint, Fill, LiveMessage, Overview, Totals } from "./types";

/** Most fills kept per account in the page; the oldest are dropped. */
export const FILL_CAP = 200;
/** How far back the equity curve reaches from its newest point, in milliseconds. */
export const CURVE_WINDOW_MS = 24 * 60 * 60 * 1000;
/** Most equity curve points kept per account in the page, whatever their spacing; the oldest are dropped. */
export const CURVE_CAP = 1500;

/** What the console knows of the accounts. Keys are lower-case addresses, in the order the bot lists them. */
export interface Book {
  /** Each account as the bot last showed it; its `curve` holds at most the newest point, except after a full read. */
  readonly accounts: ReadonlyMap<string, AccountView>;
  /** Each account's equity curve: the full read once, then every point the feed brings. */
  readonly curves: ReadonlyMap<string, readonly CurvePoint[]>;
  readonly totals: Totals | null;
  readonly fills: ReadonlyMap<string, readonly Fill[]>;
}

/** A book that knows nothing. */
export const EMPTY_BOOK: Book = { accounts: new Map(), curves: new Map(), totals: null, fills: new Map() };

/** The key an address is held under; the route may spell it in another case than the bot does. */
export function keyOf(address: string): string {
  return address.toLowerCase();
}

/** The code of an account the bot's own run halted, as opposed to one that could not start. */
const STOPPED_CODE = "account_stopped";

/**
 * The key an account is held under: its address when it trades, its label when it has no address, and its place
 * in the bot's list when it could not start: a key listed twice has an address of its own twice, and the second
 * entry must not take the trading account's place, nor an entry without an address another's.
 */
export function identityOf(
  account: { readonly address: string | null; readonly label: string | null; readonly code: string | null },
  index = 0,
): string {
  if (account.code !== null && account.code !== STOPPED_CODE) {
    return `failed:${String(index)}`;
  }
  return account.address === null ? `label:${account.label ?? ""}` : keyOf(account.address);
}

/** The bot keeps one curve point per minute, the latest read in it. */
const CURVE_STEP_MS = 60_000;

/**
 * Merges curve points into a held curve as the bot keeps it: one point per minute, the latest in it, oldest first,
 * kept to `CURVE_WINDOW_MS` before the newest point and to `CURVE_CAP` points. The feed brings a point at every
 * read, each in the minute of the last; keeping all of them would push the day's history out of the cap.
 */
export function mergeCurve(held: readonly CurvePoint[], incoming: readonly CurvePoint[]): readonly CurvePoint[] {
  if (incoming.length === 0) {
    return held;
  }
  const byMinute = new Map<number, CurvePoint>();
  for (const point of [...held, ...incoming]) {
    const minute = Math.floor(point.timeMs / CURVE_STEP_MS);
    const kept = byMinute.get(minute);
    if (kept === undefined || point.timeMs >= kept.timeMs) {
      byMinute.set(minute, point);
    }
  }
  const sorted = [...byMinute.values()].sort((a, b) => a.timeMs - b.timeMs);
  const newest = sorted.at(-1)?.timeMs ?? 0;
  return sorted.filter((point) => point.timeMs >= newest - CURVE_WINDOW_MS).slice(-CURVE_CAP);
}

/** Adds points to the curve held under `key`. */
export function withCurve(book: Book, key: string, points: readonly CurvePoint[]): Book {
  return { ...book, curves: new Map(book.curves).set(key, mergeCurve(book.curves.get(key) ?? [], points)) };
}

/**
 * Takes the feed's opening snapshot: the accounts and totals replace what was known, fills and curves of kept
 * accounts stay; curves are held only for the accounts in `shown`, when given.
 */
export function withSnapshot(book: Book, overview: Overview, shown: ReadonlySet<string> | null = null): Book {
  const accounts = new Map(overview.accounts.map((account, index) => [identityOf(account, index), account]));
  const fills = new Map([...book.fills].filter(([key]) => accounts.has(key)));
  const kept = new Map([...book.curves].filter(([key]) => accounts.has(key) && (shown?.has(key) ?? true)));
  let next: Book = { accounts, curves: kept, totals: overview.totals, fills };
  for (const [index, account] of overview.accounts.entries()) {
    const key = identityOf(account, index);
    next = shown?.has(key) ?? true ? withCurve(next, key, account.curve) : next;
  }
  return next;
}

/**
 * The key the book holds `account` under. An account that trades or has no address has a key of its own. One
 * that could not start is held by its place in the snapshot, so a read of it is matched to the listed entry with
 * its address and code; none listed, none to replace.
 */
function keyHeld(book: Book, account: AccountView): string | null {
  const key = identityOf(account);
  if (!key.startsWith("failed:")) {
    return key;
  }
  const same = [...book.accounts].find(
    ([held, view]) => held.startsWith("failed:") && view.address?.toLowerCase() === account.address?.toLowerCase() && view.label === account.label && view.code === account.code,
  );
  return same?.[0] ?? null;
}

/**
 * Takes one account's view, which replaces what the book held of it; an account that trades and the book never
 * listed is added last, and an entry that could not start is never added to what the snapshot listed.
 */
export function withAccount(book: Book, account: AccountView, shown: ReadonlySet<string> | null = null): Book {
  const key = keyHeld(book, account);
  if (key === null) {
    return book;
  }
  const next = { ...book, accounts: new Map(book.accounts).set(key, account) };
  return shown?.has(key) ?? true ? withCurve(next, key, account.curve) : next;
}

/** Takes an account's fills; fills of an account the book does not list are dropped. */
export function withFills(book: Book, address: string, fills: readonly Fill[]): Book {
  const key = keyOf(address);
  return book.accounts.has(key)
    ? { ...book, fills: new Map(book.fills).set(key, fills.slice(0, FILL_CAP)) }
    : book;
}

/**
 * Applies one feed message. Only the accounts in `shown`, those whose pages are open, have a curve held: the
 * overview draws none, and a page reads the whole curve of its account when it opens. Without `shown`, all do.
 */
export function withMessage(book: Book, message: LiveMessage, shown: ReadonlySet<string> | null = null): Book {
  switch (message.type) {
    case "snapshot":
      return withSnapshot(book, message.overview, shown);
    case "account":
      return withAccount(book, message.account, shown);
    case "fills":
      return withFills(book, message.address, message.fills);
    case "totals":
      return { ...book, totals: message.totals };
  }
}
