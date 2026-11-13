/** What the bot's funding routes answer, as the console reads it. Amounts are decimals in whole units. */

/** The class an account declares. */
export type EligibilityClass = "professional" | "eligible_client";

/** The classes, in the order the page offers them. */
export const CLASSES: readonly EligibilityClass[] = ["professional", "eligible_client"];

/** A withdrawal waiting in the node's queue. */
export interface QueuedWithdrawal {
  readonly position: number;
  readonly remaining: string;
}

/** An approved withdrawal the vault has not paid. */
export interface UnpaidPayout {
  readonly nonce: number;
  readonly amount: string;
  readonly authorized: boolean;
}

/** One account's funding. An account that could not be read carries `error` and no figures. */
export interface FundingAccount {
  readonly address: string;
  readonly error: string | null;
  readonly declared: EligibilityClass | null;
  readonly walletEth: string;
  readonly walletUsdc: string;
  readonly walletMonkey: string;
  /** The balance in the node; null before its first credit. */
  readonly balance: string | null;
  readonly unclaimed: string;
  readonly unclaimedNewest: string;
  readonly queued: readonly QueuedWithdrawal[];
  readonly unpaid: readonly UnpaidPayout[];
  readonly payoutsLoaded: boolean;
  /** An action of the account's has not ended. */
  readonly busy: boolean;
  /** The bot has not read the account yet: the figures are placeholders. */
  readonly reading: boolean;
}

/** The states of a job. */
export type JobState = "waiting" | "running" | "landed" | "failed";

/** Where a transaction a job sent stands. */
export type TxState = "sent" | "mined" | "reverted";

/** A transaction a job sent. */
export interface FundingTx {
  readonly hash: string;
  /** What it does. */
  readonly what: string;
  readonly state: TxState;
}

/** One action for one account. */
export interface FundingJob {
  readonly id: number;
  readonly action: "declare" | "deposit" | "claim" | "withdraw";
  readonly account: string;
  readonly state: JobState;
  /** What it came to, or why it failed. */
  readonly detail: string;
  /** The transactions it has sent so far, in order. */
  readonly txs: readonly FundingTx[];
}

/** The funding page's whole answer. */
export type Funding =
  | { readonly enabled: false }
  | {
      readonly enabled: true;
      /** Whether the config names a funding key: deposits may come from its wallet, and it pays gas. */
      readonly fundingKey: boolean;
      /** The statement of each class, which the bot checks a declaration against. */
      readonly statements: Readonly<Record<EligibilityClass, string>>;
      readonly accounts: readonly FundingAccount[];
      readonly jobs: readonly FundingJob[];
    };

/** Where a deposit comes from. */
export type DepositFrom = "account" | "funding";

/** Whether a job has ended. */
export function ended(job: FundingJob): boolean {
  return job.state === "landed" || job.state === "failed";
}
