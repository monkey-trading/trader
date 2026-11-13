/** The overview: every account's figures in one table, with the bot's totals. */
import type { ReactNode } from "react";

import type { AccountView } from "@/api/types";
import { useLive } from "@/components/app-context";
import { DayStopNote } from "@/components/DayStopNote";
import { CELL, DataTable } from "@/components/DataTable";
import { Link } from "@/components/Link";
import { PageTitle } from "@/components/PageTitle";
import { Pnl } from "@/components/Pnl";
import { formatBpsPercent, formatMonkey, formatUsdc, shortAddress } from "@/lib/format";
import { identityOf } from "@/api/live-state";
import { accountPath } from "@/lib/route";

const HEAD = [
  "Account", "State", "Equity (USDC)", "Realised P&L (USDC)", "Unrealised P&L (USDC)", "Margin level",
  "Open positions", "MONKEY earned", "MONKEY claimed",
] as const;
const NUMERIC = [2, 3, 4, 5, 6, 7, 8] as const;

/** Stands where a figure is missing. */
const NONE = "—";

/** What stands for MONKEY the bot has not read from the vault yet. */
const NOT_READ = "Not read yet";

/** A right-aligned number cell. */
function Num({ children }: { readonly children: ReactNode }) {
  return <td className={`${CELL} text-right`}>{children}</td>;
}

/** Shows a nullable figure with `show`, or the placeholder. */
function maybe<T>(value: T | null, show: (value: T) => ReactNode): ReactNode {
  return value === null ? NONE : show(value);
}

/** One account's row; an account with no figures yet shows the placeholder and why. */
function AccountRow({ account }: { readonly account: AccountView }) {
  const { figures } = account;
  return (
    <tr className="border-t border-line">
      <th scope="row" className={`${CELL} font-medium`}>
        {account.address === null ? (
          // No address means no page: the label names the key source.
          <span>{account.label}</span>
        ) : (
          <Link to={accountPath(account.address)}>{shortAddress(account.address)}</Link>
        )}
        {account.error === null ? null : (
          <span className="ml-2 text-loss">{`${account.code === null ? "Read failed: " : ""}${account.error}`}</span>
        )}
      </th>
      <td className="min-w-48 px-2 py-1">
        {account.halt === null ? account.state : `${account.state} (${account.halt})`}
        {account.dayStop === null ? null : <DayStopNote dayStop={account.dayStop} />}
      </td>
      <Num>{maybe(figures, (f) => formatUsdc(f.equity))}</Num>
      <Num>{maybe(figures, (f) => <Pnl value={f.realisedPnl} />)}</Num>
      <Num>{maybe(figures, (f) => <Pnl value={f.unrealisedPnl} />)}</Num>
      <Num>{maybe(figures?.marginLevelBps ?? null, formatBpsPercent)}</Num>
      <Num>{maybe(figures, (f) => f.positions.length)}</Num>
      <Num>{maybe(figures, (f) => formatMonkey(f.monkey.newestEntitled))}</Num>
      <Num>{maybe(figures, (f) => (f.monkey.claimed === null ? NOT_READ : formatMonkey(f.monkey.claimed)))}</Num>
    </tr>
  );
}

/** Lists the accounts and their totals. */
export function OverviewPage() {
  const { accounts, totals, loaded } = useLive();
  const positions = accounts.reduce((sum, account) => sum + (account.figures?.positions.length ?? 0), 0);
  return (
    <>
      <PageTitle>Accounts</PageTitle>
      {!loaded || totals === null ? (
        <p className="text-muted">Loading accounts…</p>
      ) : accounts.length === 0 ? (
        <p className="text-muted">The bot has no accounts.</p>
      ) : (
        <div>
          <DataTable label="Accounts" head={HEAD} numeric={NUMERIC}>
            {accounts.map((account, index) => (
              <AccountRow key={identityOf(account, index)} account={account} />
            ))}
            <tr className="border-t-2 border-line font-semibold">
              <th scope="row" className={CELL}>Total</th>
              <td className={CELL} />
              <Num>{formatUsdc(totals.equity)}</Num>
              <Num><Pnl value={totals.realisedPnl} /></Num>
              <Num><Pnl value={totals.unrealisedPnl} /></Num>
              <Num>{maybe(totals.marginLevelBps, formatBpsPercent)}</Num>
              <Num>{positions}</Num>
              <Num>{formatMonkey(totals.monkey.newestEntitled)}</Num>
              <Num>{totals.monkey.claimed === null ? NOT_READ : formatMonkey(totals.monkey.claimed)}</Num>
            </tr>
          </DataTable>
          <p className="mt-2 text-sm text-muted">
            {`${String(totals.reporting)} of ${String(totals.accounts)} accounts reporting.`}
          </p>
        </div>
      )}
    </>
  );
}
