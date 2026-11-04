/** The funding views' values. Every amount is a decimal string exactly as the bot serves it. */

/** The class an account declares; the bot serves the statement each one signs. */
export type DeclarationClass = "professional" | "eligible_client";

/** One declaration the account may make, with the complete statement the user reads. */
export interface DeclarationOption {
  readonly class: DeclarationClass;
  readonly statement: string;
}

/** A withdrawal waiting in the node's queue. */
export interface QueuedWithdrawal {
  /** Its place in the whole queue, from 1. */
  readonly position: number;
  /** USDC requested, and the part not yet approved. */
  readonly amount: string;
  readonly remaining: string;
}

/** An approved withdrawal the vault has not yet paid. */
export interface UnpaidApproval {
  readonly nonce: number;
  /** USDC approved. */
  readonly amount: string;
}

/** What one account holds and owes, on chain and in the node. */
export interface FundingAccount {
  readonly address: string;
  readonly name: string;
  /** The class the account declared; null until it has. */
  readonly declared: DeclarationClass | null;
  readonly walletUsdc: string;
  readonly walletEth: string;
  /** The account's balance in the node, in USDC; null before its first credit. */
  readonly balance: string | null;
  /** MONKEY the vault would mint to the account now. */
  readonly unclaimedMonkey: string;
  readonly queued: readonly QueuedWithdrawal[];
  readonly unpaid: readonly UnpaidApproval[];
}

/** What a transaction did, in the order it asks for. */
export type OperationKind = "declare" | "deposit" | "claim" | "withdraw";

/** Whether an operation is still running, landed, or failed. */
export type OperationState = "pending" | "landed" | "failed";

/** One action for one account, from the click until it lands. */
export interface Operation {
  readonly id: string;
  readonly kind: OperationKind;
  readonly address: string;
  readonly state: OperationState;
  /** What a pending operation waits for, such as "Waiting for the credit in the log". */
  readonly stage: string;
  /** The transaction hash once one was sent. */
  readonly txHash: string | null;
  /** Why a failed operation failed, such as a revert's reason. */
  readonly reason: string | null;
}

/** The funding page's read: accounts, the declarations on offer and the recent operations. */
export interface FundingRead {
  readonly accounts: readonly FundingAccount[];
  readonly declarations: readonly DeclarationOption[];
  /** The address that pays gas for claims and withdrawals; null when each account pays its own. */
  readonly gasPayer: string | null;
  /** Newest first. */
  readonly operations: readonly Operation[];
}
