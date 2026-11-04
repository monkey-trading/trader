/** Totals across accounts, summed from the bot's decimal strings as exact integers. */
import { sumUnits, unitsToText } from "./decimal";

/** The figures the overview totals. Each is null if any account's figure could not be read. */
export interface Totals {
  readonly equity: string | null;
  readonly realisedPnl: string | null;
  readonly unrealisedPnl: string | null;
  readonly openPositions: number;
  readonly monkeyEarned: string | null;
  readonly monkeyClaimed: string | null;
}

/** The slice of an account the totals read. */
export interface Summable {
  readonly equity: string;
  readonly realisedPnl: string;
  readonly unrealisedPnl: string;
  readonly openPositions: number;
  readonly monkeyEarned: string;
  readonly monkeyClaimed: string;
}

/** A sum as a decimal string with every digit the units hold, or null. */
function exact(values: readonly string[]): string | null {
  const units = sumUnits(values);
  return units === null ? null : unitsToText(units, 18).replace(/\.?0+$/, "");
}

/** Adds the accounts' figures; an empty list totals zero. */
export function totalsOf(accounts: readonly Summable[]): Totals {
  return {
    equity: exact(accounts.map((a) => a.equity)),
    realisedPnl: exact(accounts.map((a) => a.realisedPnl)),
    unrealisedPnl: exact(accounts.map((a) => a.unrealisedPnl)),
    openPositions: accounts.reduce((sum, a) => sum + a.openPositions, 0),
    monkeyEarned: exact(accounts.map((a) => a.monkeyEarned)),
    monkeyClaimed: exact(accounts.map((a) => a.monkeyClaimed)),
  };
}
