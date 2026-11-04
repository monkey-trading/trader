/** Every account's declaration, wallet, balance, MONKEY and withdrawals, with a checkbox to select it. */
import type { FundingAccount } from "@/api/funding-types";
import { formatTrimmed, shortAddress } from "@/lib/format";
import { withdrawalSummary } from "@/lib/funding";

import { CELL, DataTable } from "./DataTable";

interface FundingTableProps {
  readonly accounts: readonly FundingAccount[];
  readonly selected: ReadonlySet<string>;
  readonly onToggle: (address: string) => void;
}

const HEAD = [
  "Select", "Account", "Declaration", "Wallet USDC", "Wallet ETH", "Balance (USDC)", "Unclaimed MONKEY", "Withdrawals",
] as const;
const NUMERIC = [3, 4, 5, 6] as const;
const NOT_YET = "not declared";

/** Words for a declared class. */
function declarationText(account: FundingAccount): string {
  switch (account.declared) {
    case null:
      return NOT_YET;
    case "professional":
      return "professional client";
    case "eligible_client":
      return "eligible client";
  }
}

/** The accounts as a table whose first column selects. */
export function FundingTable({ accounts, selected, onToggle }: FundingTableProps) {
  return (
    <DataTable label="Account funds" head={HEAD} numeric={NUMERIC}>
      {accounts.map((account) => (
        <tr key={account.address} className="border-t border-line">
          <td className={CELL}>
            <input
              type="checkbox"
              className="size-5 accent-accent"
              aria-label={`Select ${account.name}`}
              checked={selected.has(account.address)}
              onChange={() => {
                onToggle(account.address);
              }}
            />
          </td>
          <th scope="row" className={`${CELL} font-medium`}>
            {account.name}
            <span className="ml-2 text-muted">{shortAddress(account.address)}</span>
          </th>
          <td className={CELL}>{declarationText(account)}</td>
          <td className={`${CELL} text-right`}>{formatTrimmed(account.walletUsdc, 6)}</td>
          <td className={`${CELL} text-right`}>{formatTrimmed(account.walletEth, 18)}</td>
          <td className={`${CELL} text-right`}>{account.balance === null ? "—" : formatTrimmed(account.balance, 6)}</td>
          <td className={`${CELL} text-right`}>{formatTrimmed(account.unclaimedMonkey, 18)}</td>
          <td className={CELL}>{withdrawalSummary(account)}</td>
        </tr>
      ))}
    </DataTable>
  );
}
