/** Pure rules of the funding views: amounts the user types, who an action applies to, what was confirmed. */
import type { DeclarationClass, FundingAccount, Operation, OperationKind } from "@/api/funding-types";

import { formatTrimmed } from "./format";
import { isDecimal, signOf, sumUnits, unitsToText } from "./decimal";

/** USDC has six fractional digits. */
const USDC_PLACES = 6;

/** The most operations the page lists for one kind. */
export const OPERATIONS_SHOWN = 8;

/** An amount the user typed, checked, or why it was not accepted. */
export type TypedAmount =
  | { readonly status: "ok"; readonly value: string }
  | { readonly status: "invalid"; readonly message: string };

/** Checks a typed USDC amount: a plain positive decimal with at most six fractional digits. Bounds are the bot's. */
export function parseUsdcAmount(input: string): TypedAmount {
  const value = input.trim();
  if (value === "") {
    return { status: "invalid", message: "Enter an amount in USDC." };
  }
  if (!isDecimal(value) || value.startsWith("-")) {
    return { status: "invalid", message: "Enter a plain positive number such as 250 or 12.5." };
  }
  const fraction = value.split(".")[1] ?? "";
  if (fraction.length > USDC_PLACES) {
    return { status: "invalid", message: `USDC has at most ${String(USDC_PLACES)} decimal places.` };
  }
  if (signOf(value) === 0) {
    return { status: "invalid", message: "The amount must be above zero." };
  }
  return { status: "ok", value };
}

/** The accounts of `accounts` whose address is in `selected`, in the list's order. */
export function selectedAccounts(accounts: readonly FundingAccount[], selected: ReadonlySet<string>): readonly FundingAccount[] {
  return accounts.filter((account) => selected.has(account.address));
}

/** The selected accounts that have not declared. */
export function undeclared(accounts: readonly FundingAccount[]): readonly FundingAccount[] {
  return accounts.filter((account) => account.declared === null);
}

/** The selected accounts with MONKEY to claim. */
export function withUnclaimed(accounts: readonly FundingAccount[]): readonly FundingAccount[] {
  return accounts.filter((account) => signOf(account.unclaimedMonkey) > 0);
}

/**
 * What the user confirmed: one class for exactly these accounts. A change to either makes an earlier
 * confirmation stale, so the key differs and the box reads unchecked.
 */
export function confirmationKey(declaration: DeclarationClass, accounts: readonly FundingAccount[]): string {
  return `${declaration}:${accounts.map((account) => account.address).sort().join(",")}`;
}

/** The operations of `kind`, newest first, bounded. */
export function operationsOfKind(operations: readonly Operation[], kind: OperationKind): readonly Operation[] {
  return operations.filter((operation) => operation.kind === kind).slice(0, OPERATIONS_SHOWN);
}

/** Whether an operation of `kind` for `address` is still running. */
export function isBusy(operations: readonly Operation[], kind: OperationKind, address: string): boolean {
  return operations.some((operation) => operation.kind === kind && operation.address === address && operation.state === "pending");
}

/** The words for an operation's state beside its account. */
export function operationStatus(operation: Operation): string {
  switch (operation.state) {
    case "pending":
      return `${operation.stage}…`;
    case "landed":
      return "Landed";
    case "failed":
      return `Failed: ${operation.reason ?? "no reason given"}`;
  }
}

/** What the account has in the withdrawal queue and awaiting payment, as one line; a dash when nothing. */
export function withdrawalSummary(account: FundingAccount): string {
  const parts: string[] = [];
  if (account.queued.length > 0) {
    const remaining = sumUnits(account.queued.map((entry) => entry.remaining));
    const amount = remaining === null ? "unavailable" : formatTrimmed(unitsToText(remaining, USDC_PLACES), USDC_PLACES);
    parts.push(`${String(account.queued.length)} queued (${amount} USDC unapproved)`);
  }
  if (account.unpaid.length > 0) {
    const approved = sumUnits(account.unpaid.map((entry) => entry.amount));
    const amount = approved === null ? "unavailable" : formatTrimmed(unitsToText(approved, USDC_PLACES), USDC_PLACES);
    parts.push(`${String(account.unpaid.length)} approved, unpaid (${amount} USDC)`);
  }
  return parts.length === 0 ? "—" : parts.join("; ");
}
