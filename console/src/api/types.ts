/** The values the console shows, as the bot's console API serves them. Amounts are decimal strings exactly as served. */

/** Where an account stands. */
export type AccountState = "starting" | "running" | "stopped" | "flattening" | "halted";

/** An open position; a negative quantity is short. */
export interface Position {
  readonly market: number;
  /** The market's symbol, when the node names it. */
  readonly symbol: string | null;
  readonly qty: string;
  readonly entry: string;
  readonly mark: string | null;
  readonly unrealisedPnl: string | null;
  readonly closeout: Closeout | null;
}

/** Where an account closes out as one position's market moves, as the node works it out. */
export interface Closeout {
  /** `due`: already below the close-out level; `at`: at `price`; `beyond`: no move the node considers. */
  readonly reach: "due" | "at" | "beyond";
  readonly price: string | null;
}

/** A resting order. */
export interface Order {
  readonly orderId: number;
  readonly market: number;
  readonly side: "buy" | "sell";
  readonly qty: string;
  readonly kind: { readonly type: "limit"; readonly price: string } | { readonly type: "market"; readonly quoted: string };
  readonly reduceOnly: boolean;
}

/** MONKEY in token base units (18 decimals), as digit strings. */
export interface Monkey {
  /** The cumulative entitlement the vault can check a claim against now. */
  readonly entitled: string;
  /** The cumulative entitlement at the node's newest checkpoint. */
  readonly newestEntitled: string;
  /** What the vault has paid; null until the bot reads it. */
  readonly claimed: string | null;
}

/** An account's figures as the node gave them, in USDC unless named otherwise. */
export interface Figures {
  readonly balance: string;
  readonly equity: string;
  readonly margin: string;
  /** Equity over the margin requirement in basis points; null with nothing required. */
  readonly marginLevelBps: number | null;
  readonly unrealisedPnl: string;
  readonly realisedPnl: string;
  readonly positions: readonly Position[];
  readonly orders: readonly Order[];
  readonly monkey: Monkey;
  /** The node's time of the read, Unix milliseconds. */
  readonly timeMs: number;
}

/** One adjustable parameter of a strategy: a whole number of bars, or a decimal such as a band width. */
export interface Param {
  readonly name: string;
  readonly kind: "integer" | "decimal";
  /** The value as a decimal string. */
  readonly value: string;
  /** The lowest and highest value the bot's config lets the console set; null when it lets none. */
  readonly bounds: readonly [string, string] | null;
}

/** A strategy and its state. */
export interface Strategy {
  readonly id: number;
  readonly kind: string;
  readonly market: string;
  readonly bar: string;
  readonly running: boolean;
  readonly params: readonly Param[];
  readonly warmedUp: boolean;
  /** The position the strategy last wanted: long, short or flat. */
  readonly target: string | null;
}

/** One point of the equity curve. */
export interface CurvePoint {
  /** Unix time in milliseconds. */
  readonly timeMs: number;
  readonly equity: string;
}

/** The daily loss limit having tripped: the account places no order until `untilMs`. */
export interface DayStop {
  /** When trading resumes, Unix milliseconds: the next UTC midnight. */
  readonly untilMs: number;
  /** The day's loss that tripped the limit, in USD. */
  readonly loss: string;
}

/** One account as the bot shows it. */
export interface AccountView {
  /** The account's address; null when its key would not unlock, so it has no page and no controls. */
  readonly address: string | null;
  /** Names the account's key source, such as its file; the way to tell apart accounts with no address. */
  readonly label: string | null;
  /** What kind of failure halted the account at startup, such as `key_locked`; null for an account that started. */
  readonly code: string | null;
  readonly state: AccountState;
  /** Why the account halted. */
  readonly halt: string | null;
  /** Set while the daily loss limit keeps the account from placing orders. */
  readonly dayStop: DayStop | null;
  readonly strategies: readonly Strategy[];
  /** The last figures read; null until the first read. */
  readonly figures: Figures | null;
  /** Why the last read of the node failed. */
  readonly error: string | null;
  readonly curve: readonly CurvePoint[];
}

/** A fill, newest first in lists. */
export interface Fill {
  /** Where the fill sits in the log, `<seq>:<index>`. */
  readonly cursor: string;
  /** Unix time in milliseconds. */
  readonly time: number;
  readonly market: number;
  readonly symbol: string | null;
  readonly side: "buy" | "sell";
  readonly qty: string;
  readonly price: string;
  readonly realisedPnl: string;
  readonly profitFee: string;
}

/** The figures summed over the accounts that have them. */
export interface Totals {
  readonly accounts: number;
  /** How many accounts the sums hold. */
  readonly reporting: number;
  readonly balance: string;
  readonly equity: string;
  readonly margin: string;
  readonly marginLevelBps: number | null;
  readonly unrealisedPnl: string;
  readonly realisedPnl: string;
  readonly monkey: Monkey;
}

/** Every account and their totals. */
export interface Overview {
  readonly accounts: readonly AccountView[];
  readonly totals: Totals;
}

/** What the live feed carries. */
export type LiveMessage =
  | { readonly type: "snapshot"; readonly overview: Overview }
  | { readonly type: "account"; readonly account: AccountView }
  | { readonly type: "fills"; readonly address: string; readonly fills: readonly Fill[] }
  | { readonly type: "totals"; readonly totals: Totals };
