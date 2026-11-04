/** The overview: every account's figures in one table, with totals. */
import type { ReactNode } from "react";

import type { AccountSummary } from "@/api/types";
import { useLive } from "@/components/app-context";
import { CELL, DataTable } from "@/components/DataTable";
import { Link } from "@/components/Link";
import { PageTitle } from "@/components/PageTitle";
import { Pnl } from "@/components/Pnl";
import { formatDecimal, formatRatioPercent, formatUsdc, shortAddress } from "@/lib/format";
import { accountPath } from "@/lib/route";
import { totalsOf } from "@/lib/totals";

const HEAD = [
  "Account", "State", "Equity (USDC)", "Realised P&L (USDC)", "Unrealised P&L (USDC)", "Margin level",
  "Open positions", "MONKEY earned", "MONKEY claimed",
] as const;
const NUMERIC = [2, 3, 4, 5, 6, 7, 8] as const;

/** Stands where a figure is missing. */
const NONE = "—";

/** A right-aligned number cell. */
function Num({ children }: { readonly children: ReactNode }) {
  return <td className={`${CELL} text-right`}>{children}</td>;
}

/** Shows a nullable decimal with `show`, or the placeholder. */
function maybe(value: string | null, show: (value: string) => ReactNode): ReactNode {
  return value === null ? NONE : show(value);
}

/** One account's row. */
function AccountRow({ account }: { readonly account: AccountSummary }) {
  return (
    <tr className="border-t border-line">
      <th scope="row" className={`${CELL} font-medium`}>
        <Link to={accountPath(account.address)}>{account.name}</Link>
        <span className="ml-2 text-muted">{shortAddress(account.address)}</span>
      </th>
      <td className={CELL}>{account.state}</td>
      <Num>{formatUsdc(account.equity)}</Num>
      <Num><Pnl value={account.realisedPnl} /></Num>
      <Num><Pnl value={account.unrealisedPnl} /></Num>
      <Num>{maybe(account.marginLevel, formatRatioPercent)}</Num>
      <Num>{account.openPositions}</Num>
      <Num>{formatDecimal(account.monkeyEarned, 2)}</Num>
      <Num>{formatDecimal(account.monkeyClaimed, 2)}</Num>
    </tr>
  );
}

/** Lists the accounts and their totals. */
export function OverviewPage() {
  const { accounts, loaded, connection } = useLive();
  const totals = totalsOf(accounts);
  return (
    <>
      <PageTitle>Accounts</PageTitle>
      {!loaded ? (
        <p className="text-muted">Loading accounts…</p>
      ) : accounts.length === 0 ? (
        <p className="text-muted">The bot has no accounts.</p>
      ) : (
        <div className={connection === "live" ? "" : "opacity-70"}>
          <DataTable label="Accounts" head={HEAD} numeric={NUMERIC}>
            {accounts.map((account) => (
              <AccountRow key={account.address} account={account} />
            ))}
            <tr className="border-t-2 border-line font-semibold">
              <th scope="row" className={CELL}>Total</th>
              <td className={CELL} />
              <Num>{maybe(totals.equity, formatUsdc)}</Num>
              <Num>{maybe(totals.realisedPnl, (value) => <Pnl value={value} />)}</Num>
              <Num>{maybe(totals.unrealisedPnl, (value) => <Pnl value={value} />)}</Num>
              <Num>{NONE}</Num>
              <Num>{totals.openPositions}</Num>
              <Num>{maybe(totals.monkeyEarned, (value) => formatDecimal(value, 2))}</Num>
              <Num>{maybe(totals.monkeyClaimed, (value) => formatDecimal(value, 2))}</Num>
            </tr>
          </DataTable>
        </div>
      )}
    </>
  );
}
