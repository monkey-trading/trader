/** The console's book of accounts and the pure steps that change it: a read replaces, a feed message adds. */
import type { AccountDetail, AccountList, AccountRead, AccountSummary, LiveMessage } from "./types";

/** Most fills kept per account in the page; the oldest are dropped. */
export const FILL_CAP = 200;
/** Most equity curve points kept per account in the page; the oldest are dropped. */
export const CURVE_CAP = 500;

/** What the console knows of the accounts. */
export interface Book {
  readonly summaries: ReadonlyMap<string, AccountSummary>;
  readonly details: ReadonlyMap<string, AccountDetail>;
  /** The feed position of the read each account was last taken from; older messages are ignored. */
  readonly base: ReadonlyMap<string, number>;
}

/** A book that knows nothing. */
export const EMPTY_BOOK: Book = { summaries: new Map(), details: new Map(), base: new Map() };

/** The figures of a detail, without its lists. */
function summaryOf(detail: AccountSummary): AccountSummary {
  return {
    address: detail.address,
    name: detail.name,
    state: detail.state,
    stateReason: detail.stateReason,
    balance: detail.balance,
    equity: detail.equity,
    realisedPnl: detail.realisedPnl,
    unrealisedPnl: detail.unrealisedPnl,
    marginLevel: detail.marginLevel,
    openPositions: detail.openPositions,
    monkeyEarned: detail.monkeyEarned,
    monkeyClaimed: detail.monkeyClaimed,
  };
}

/** Takes a read of the whole list: accounts the bot no longer lists go, the rest are replaced. */
export function withList(book: Book, list: AccountList): Book {
  const summaries = new Map(list.accounts.map((account) => [account.address, account]));
  const details = new Map([...book.details].filter(([address]) => summaries.has(address)));
  const base = new Map([...summaries.keys()].map((address) => [address, list.seq]));
  return { summaries, details, base };
}

/** Takes a read of one account, which replaces what the book held of it. */
export function withRead(book: Book, read: AccountRead): Book {
  const { address } = read.account;
  return {
    summaries: new Map(book.summaries).set(address, summaryOf(read.account)),
    details: new Map(book.details).set(address, read.account),
    base: new Map(book.base).set(address, read.seq),
  };
}

/** Applies one feed message; one at or before its account's read, or for a detail not held, changes only what it can. */
export function withMessage(book: Book, message: LiveMessage): Book {
  switch (message.type) {
    case "account": {
      const { address } = message.account;
      if (message.seq <= (book.base.get(address) ?? -1)) {
        return book;
      }
      const held = book.details.get(address);
      return {
        ...book,
        summaries: new Map(book.summaries).set(address, summaryOf(message.account)),
        details: held
          ? new Map(book.details).set(address, { ...message.account, fills: held.fills, curve: held.curve })
          : book.details,
      };
    }
    case "fill": {
      const held = book.details.get(message.address);
      if (!held || message.seq <= (book.base.get(message.address) ?? -1)) {
        return book;
      }
      const fills = [message.fill, ...held.fills].slice(0, FILL_CAP);
      return { ...book, details: new Map(book.details).set(message.address, { ...held, fills }) };
    }
    case "curve": {
      const held = book.details.get(message.address);
      if (!held || message.seq <= (book.base.get(message.address) ?? -1)) {
        return book;
      }
      const curve = [...held.curve, message.point].slice(-CURVE_CAP);
      return { ...book, details: new Map(book.details).set(message.address, { ...held, curve }) };
    }
  }
}
