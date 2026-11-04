/** Parsers for the bot's funding bodies. */
import { choice, count, decimal, items, nullableDecimal, nullableText, object, text, type Fields } from "@/lib/shape";

import type {
  DeclarationClass, DeclarationOption, FundingAccount, FundingRead, Operation, QueuedWithdrawal, UnpaidApproval,
} from "./funding-types";

const CLASSES = ["professional", "eligible_client"] as const satisfies readonly DeclarationClass[];

function queuedOf(fields: Fields): QueuedWithdrawal {
  return { position: count(fields, "position"), amount: decimal(fields, "amount"), remaining: decimal(fields, "remaining") };
}

function unpaidOf(fields: Fields): UnpaidApproval {
  return { nonce: count(fields, "nonce"), amount: decimal(fields, "amount") };
}

function accountOf(fields: Fields): FundingAccount {
  const declared = fields["declared"];
  return {
    address: text(fields, "address"),
    name: text(fields, "name"),
    declared: declared === null ? null : choice(fields, "declared", CLASSES),
    walletUsdc: decimal(fields, "walletUsdc"),
    walletEth: decimal(fields, "walletEth"),
    balance: nullableDecimal(fields, "balance"),
    unclaimedMonkey: decimal(fields, "unclaimedMonkey"),
    queued: items(fields, "queued", queuedOf),
    unpaid: items(fields, "unpaid", unpaidOf),
  };
}

function declarationOf(fields: Fields): DeclarationOption {
  return { class: choice(fields, "class", CLASSES), statement: text(fields, "statement") };
}

function operationOf(fields: Fields): Operation {
  return {
    id: text(fields, "id"),
    kind: choice(fields, "kind", ["declare", "deposit", "claim", "withdraw"]),
    address: text(fields, "address"),
    state: choice(fields, "state", ["pending", "landed", "failed"]),
    stage: text(fields, "stage"),
    txHash: nullableText(fields, "txHash"),
    reason: nullableText(fields, "reason"),
  };
}

/** Parses the funding read, which is also what each funding action returns. */
export function readFunding(body: unknown): FundingRead {
  const fields = object(body, "funding body");
  return {
    accounts: items(fields, "accounts", accountOf),
    declarations: items(fields, "declarations", declarationOf),
    gasPayer: nullableText(fields, "gasPayer"),
    operations: items(fields, "operations", operationOf),
  };
}
