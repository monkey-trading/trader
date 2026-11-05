/** The console's book of accounts and the pure steps that change it: the feed's messages and the REST reads. */
import type { AccountView, Fill, LiveMessage, Overview, Totals } from "./types";

/** Most fills kept per account in the page; the oldest are dropped. */
export const FILL_CAP = 200;
/** Most equity curve points kept per account in the page; the oldest are dropped. */
export const CURVE_CAP = 2000;

/** What the console knows of the accounts. Keys are lower-case addresses, in the order the bot lists them. */
export interface Book {
  readonly accounts: ReadonlyMap<string, AccountView>;
  readonly totals: Totals | null;
  readonly fills: ReadonlyMap<string, readonly Fill[]>;
}

/** A book that knows nothing. */
export const EMPTY_BOOK: Book = { accounts: new Map(), totals: null, fills: new Map() };

/** The key an address is held under; the route may spell it in another case than the bot does. */
export function keyOf(address: string): string {
  return address.toLowerCase();
}

/** An account's curve, capped to the newest points. */
function capped(account: AccountView): AccountView {
  return account.curve.length > CURVE_CAP ? { ...account, curve: account.curve.slice(-CURVE_CAP) } : account;
}

/** Takes the feed's opening snapshot: the accounts and totals replace what was known, fills of kept accounts stay. */
export function withSnapshot(book: Book, overview: Overview): Book {
  const accounts = new Map(overview.accounts.map((account) => [keyOf(account.address), capped(account)]));
  const fills = new Map([...book.fills].filter(([key]) => accounts.has(key)));
  return { accounts, totals: overview.totals, fills };
}

/** Takes one account's view, which replaces what the book held of it; an account the book never listed is added last. */
export function withAccount(book: Book, account: AccountView): Book {
  return { ...book, accounts: new Map(book.accounts).set(keyOf(account.address), capped(account)) };
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
