/** The accounts' funding in one table, with a checkbox to select each. */
import type { FundingAccount } from "@/api/funding-types";
import { formatTrimmed, shortAddress } from "@/lib/format";

import { CELL, DataTable } from "./DataTable";

const HEAD = [
  "Select", "Account", "Declaration", "Wallet ETH", "Wallet USDC", "Wallet MONKEY", "Balance (USDC)",
  "Unclaimed MONKEY", "Withdrawals",
] as const;
const NUMERIC = [3, 4, 5, 6, 7] as const;
const NONE = "—";
const LOADING = "Loading payouts…";

/** What the account's withdrawals come to, in words. */
function withdrawals(account: FundingAccount): string {
  const parts = [
    ...account.queued.map((q) => `${formatTrimmed(q.remaining, 6)} queued at ${String(q.position)}`),
    ...account.unpaid.map(
      (p) => `${formatTrimmed(p.amount, 6)} ${p.authorized ? "approved, unpaid" : "awaiting authorization"}`,
    ),
  ];
  if (parts.length === 0) {
    // An unfinished read of the payouts is not an empty queue.
    return account.payoutsLoaded ? NONE : LOADING;
  }
  return account.payoutsLoaded ? parts.join("; ") : `${parts.join("; ")} (more may still load)`;
}

function Row({ account, checked, onToggle }: {
  readonly account: FundingAccount;
  readonly checked: boolean;
  readonly onToggle: () => void;
}) {
  const label = `Select ${account.address}`;
  return (
    <tr className="border-t border-line">
      <td className={CELL}>
        <input type="checkbox" aria-label={label} checked={checked} onChange={onToggle} className="size-5" />
      </td>
      <th scope="row" className={`${CELL} font-medium`}>
        <span title={account.address}>{shortAddress(account.address)}</span>
        {account.busy ? <span className="ml-2 text-muted">working</span> : null}
      </th>
      {account.reading ? (
        <td colSpan={HEAD.length - 2} className="px-2 py-1 text-muted">Reading…</td>
      ) : account.error === null ? (
        <>
          <td className={CELL}>{account.declared === null ? "Not declared" : account.declared.replace("_", " ")}</td>
          <td className={`${CELL} text-right`}>{formatTrimmed(account.walletEth, 6)}</td>
          <td className={`${CELL} text-right`}>{formatTrimmed(account.walletUsdc, 6)}</td>
          <td className={`${CELL} text-right`}>{formatTrimmed(account.walletMonkey, 4)}</td>
          <td className={`${CELL} text-right`}>{account.balance === null ? "Never credited" : formatTrimmed(account.balance, 6)}</td>
          <td className={`${CELL} text-right`}>
            {formatTrimmed(account.unclaimed, 4)}
            {account.unclaimedNewest === account.unclaimed ? "" : ` (${formatTrimmed(account.unclaimedNewest, 4)} next)`}
          </td>
          <td className="px-2 py-1">{withdrawals(account)}</td>
        </>
      ) : (
        <td colSpan={HEAD.length - 2} className="px-2 py-1 text-loss">{`Not read: ${account.error}`}</td>
      )}
    </tr>
  );
}

/** Lists the accounts; `selected` holds the addresses ticked. */
export function FundingTable({ accounts, selected, onToggle, onToggleAll }: {
  readonly accounts: readonly FundingAccount[];
  readonly selected: ReadonlySet<string>;
  readonly onToggle: (address: string) => void;
  readonly onToggleAll: () => void;
}) {
  const all = accounts.length > 0 && accounts.every((a) => selected.has(a.address));
  return (
    <>
      <label className="mb-2 flex items-center gap-2">
        <input type="checkbox" checked={all} onChange={onToggleAll} className="size-5" />
        Select every account
      </label>
      <DataTable label="Funding of each account" head={HEAD} numeric={NUMERIC}>
        {accounts.map((account) => (
          <Row
            key={account.address}
            account={account}
            checked={selected.has(account.address)}
            onToggle={() => { onToggle(account.address); }}
          />
        ))}
      </DataTable>
    </>
  );
}
