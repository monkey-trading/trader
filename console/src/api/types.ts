/** The values the console shows. Every amount is a decimal string exactly as the bot serves it. */

/** Whether an account trades. */
export type AccountState = "running" | "stopped" | "halted";

/** Whether a strategy trades. */
export type StrategyState = "running" | "stopped" | "failed";

/** One account's figures, in USDC unless named otherwise. */
export interface AccountSummary {
  readonly address: string;
  readonly name: string;
  readonly state: AccountState;
  /** Why the account is stopped or halted, when the bot says. */
  readonly stateReason: string | null;
  readonly balance: string;
  readonly equity: string;
  readonly realisedPnl: string;
  readonly unrealisedPnl: string;
  /** Equity over the margin requirement as a ratio; null while no position is open. */
  readonly marginLevel: string | null;
  readonly openPositions: number;
  /** MONKEY earned and claimed, in whole MONKEY. */
  readonly monkeyEarned: string;
  readonly monkeyClaimed: string;
}

/** An open position; a negative quantity is short. */
export interface Position {
  readonly market: string;
  readonly qty: string;
  readonly entry: string;
  readonly mark: string | null;
  readonly unrealisedPnl: string;
  readonly closeout: string | null;
}

/** A resting order. */
export interface Order {
  readonly orderId: string;
  readonly market: string;
  readonly side: "buy" | "sell";
  readonly qty: string;
  readonly price: string | null;
  readonly reduceOnly: boolean;
}

/** A fill, newest first in lists. */
export interface Fill {
  readonly id: string;
  /** Unix time in milliseconds. */
  readonly time: number;
  readonly market: string;
  readonly side: "buy" | "sell";
  readonly qty: string;
  readonly price: string;
  readonly fee: string;
  readonly realisedPnl: string;
}

/** One point of the equity curve. */
export interface CurvePoint {
  /** Unix time in milliseconds. */
  readonly time: number;
  readonly equity: string;
}

/** A strategy parameter and the bounds the bot's config sets for it. */
export interface Param {
  readonly name: string;
  readonly kind: "integer" | "decimal";
  readonly value: string;
  readonly min: string;
  readonly max: string;
}

/** A strategy and its state. */
export interface Strategy {
  readonly id: string;
  readonly kind: string;
  readonly market: string;
  readonly bar: string;
  readonly state: StrategyState;
  readonly reason: string | null;
  readonly params: readonly Param[];
}

/** An account's page: its figures, positions, orders, fills, equity curve and strategies. */
export interface AccountDetail extends AccountSummary {
  readonly positions: readonly Position[];
  readonly orders: readonly Order[];
  readonly strategies: readonly Strategy[];
  readonly fills: readonly Fill[];
  readonly curve: readonly CurvePoint[];
}

/** What the live feed carries after the first read. */
export type LiveMessage =
  | { readonly type: "account"; readonly seq: number; readonly account: Omit<AccountDetail, "fills" | "curve"> }
  | { readonly type: "fill"; readonly seq: number; readonly address: string; readonly fill: Fill }
  | { readonly type: "curve"; readonly seq: number; readonly address: string; readonly point: CurvePoint };

/** A read of the accounts list, with the feed position it was taken at. */
export interface AccountList {
  readonly seq: number;
  readonly accounts: readonly AccountSummary[];
}

/** A read of one account, with the feed position it was taken at. */
export interface AccountRead {
  readonly seq: number;
  readonly account: AccountDetail;
}
