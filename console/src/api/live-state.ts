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

/** The key an account is held under: its address when it has one, its label otherwise. */
export function identityOf(account: { readonly address: string | null; readonly label: string | null }): string {
  return account.address === null ? `label:${account.label ?? ""}` : keyOf(account.address);
}

/**
 * Merges curve points into a held curve: one point per time (the incoming one wins), oldest first, kept to
 * `CURVE_WINDOW_MS` before the newest point and to `CURVE_CAP` points.
 */
export function mergeCurve(held: readonly CurvePoint[], incoming: readonly CurvePoint[]): readonly CurvePoint[] {
  if (incoming.length === 0) {
    return held;
  }
  const byTime = new Map(held.map((point) => [point.timeMs, point]));
  for (const point of incoming) {
    byTime.set(point.timeMs, point);
  }
  const sorted = [...byTime.values()].sort((a, b) => a.timeMs - b.timeMs);
  const newest = sorted.at(-1)?.timeMs ?? 0;
  return sorted.filter((point) => point.timeMs >= newest - CURVE_WINDOW_MS).slice(-CURVE_CAP);
}

/** Adds points to the curve held under `key`. */
export function withCurve(book: Book, key: string, points: readonly CurvePoint[]): Book {
  return { ...book, curves: new Map(book.curves).set(key, mergeCurve(book.curves.get(key) ?? [], points)) };
}

/** Takes the feed's opening snapshot: the accounts and totals replace what was known, fills and curves of kept accounts stay. */
export function withSnapshot(book: Book, overview: Overview): Book {
  const accounts = new Map(overview.accounts.map((account) => [identityOf(account), account]));
  const fills = new Map([...book.fills].filter(([key]) => accounts.has(key)));
  const kept = new Map([...book.curves].filter(([key]) => accounts.has(key)));
  let next: Book = { accounts, curves: kept, totals: overview.totals, fills };
  for (const account of overview.accounts) {
    next = withCurve(next, identityOf(account), account.curve);
  }
  return next;
}

/** Takes one account's view, which replaces what the book held of it; an account the book never listed is added last. */
export function withAccount(book: Book, account: AccountView): Book {
  const key = identityOf(account);
  return withCurve({ ...book, accounts: new Map(book.accounts).set(key, account) }, key, account.curve);
}

/** Takes an account's fills; fills of an account the book does not list are dropped. */
export function withFills(book: Book, address: string, fills: readonly Fill[]): Book {
  const key = keyOf(address);
  return book.accounts.has(key)
    ? { ...book, fills: new Map(book.fills).set(key, fills.slice(0, FILL_CAP)) }
    : book;
}

/** Applies one feed message. */
export function withMessage(book: Book, message: LiveMessage): Book {
  switch (message.type) {
    case "snapshot":
      return withSnapshot(book, message.overview);
    case "account":
      return withAccount(book, message.account);
    case "fills":
      return withFills(book, message.address, message.fills);
    case "totals":
      return { ...book, totals: message.totals };
  }
}
