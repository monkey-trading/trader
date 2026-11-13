/** Parsers for the bot's funding answers. */
import {
  choice, count, decimal, flag, items, list, note, nullableDecimal, nullableNote, object, ShapeError, text, type Fields,
} from "@/lib/shape";

import { CLASSES, type EligibilityClass, type Funding, type FundingAccount, type FundingJob } from "./funding-types";

const JOB_STATES = ["waiting", "running", "landed", "failed"] as const;
const ACTIONS = ["declare", "deposit", "claim", "withdraw"] as const;

function classOf(fields: Fields, key: string): EligibilityClass {
  return choice(fields, key, CLASSES);
}

function accountOf(fields: Fields): FundingAccount {
  const address = text(fields, "address");
  const error = nullableNote(fields, "error");
  const busy = fields["busy"] === undefined ? false : flag(fields, "busy");
  if (error !== null) {
    return {
      address, error, declared: null, walletEth: "0", walletUsdc: "0", walletMonkey: "0", balance: null,
      unclaimed: "0", unclaimedNewest: "0", queued: [], unpaid: [], payoutsLoaded: false, busy,
    };
  }
  return {
    address,
    error,
    declared: fields["declared"] === null ? null : classOf(fields, "declared"),
    walletEth: decimal(fields, "wallet_eth"),
    walletUsdc: decimal(fields, "wallet_usdc"),
    walletMonkey: decimal(fields, "wallet_monkey"),
    balance: nullableDecimal(fields, "balance"),
    unclaimed: decimal(fields, "unclaimed"),
    unclaimedNewest: decimal(fields, "unclaimed_newest"),
    queued: items(fields, "queued", (q) => ({ position: count(q, "position"), remaining: decimal(q, "remaining") })),
    unpaid: items(fields, "unpaid", (p) => ({
      nonce: count(p, "nonce"), amount: decimal(p, "amount"), authorized: flag(p, "authorized"),
    })),
    payoutsLoaded: flag(fields, "payouts_loaded"),
    busy,
  };
}

function jobOf(fields: Fields): FundingJob {
  return {
    id: count(fields, "id"),
    action: choice(fields, "action", ACTIONS),
    account: text(fields, "account"),
    state: choice(fields, "state", JOB_STATES),
    detail: note(fields, "detail"),
    txs: list(fields["txs"], "txs").map((tx) => {
      if (typeof tx !== "string" || tx.length > 100) {
        throw new ShapeError("a transaction is not a hash");
      }
      return tx;
    }),
  };
}

/** The funding read. */
export function readFunding(body: unknown): Funding {
  const fields = object(body, "funding");
  if (!flag(fields, "enabled")) {
    return { enabled: false };
  }
  const statements = object(fields["statements"], "statements");
  return {
    enabled: true,
    fundingKey: flag(fields, "funding_key"),
    statements: { professional: text(statements, "professional"), eligible_client: text(statements, "eligible_client") },
    accounts: items(fields, "accounts", accountOf),
    jobs: items(fields, "jobs", jobOf),
  };
}

/** The jobs an action request started. */
export function readJobs(body: unknown): readonly FundingJob[] {
  return items(object(body, "jobs answer"), "jobs", jobOf);
}
