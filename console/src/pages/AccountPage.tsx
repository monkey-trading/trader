/** One account: figures, controls, equity curve, strategies, positions, orders and fills. */
import { useEffect, type ReactNode } from "react";

import type { AccountDetail } from "@/api/types";
import { AccountControls } from "@/components/AccountControls";
import { useLive, useServices } from "@/components/app-context";
import { EquityCurve } from "@/components/EquityCurve";
import { Link } from "@/components/Link";
import { PageTitle } from "@/components/PageTitle";
import { Pnl } from "@/components/Pnl";
import { FillsTable, OrdersTable, PositionsTable } from "@/components/PositionsTable";
import { StrategyCard } from "@/components/StrategyCard";
import { formatDecimal, formatRatioPercent, formatUsdc } from "@/lib/format";

/** A labelled figure. */
function Figure({ label, children }: { readonly label: string; readonly children: ReactNode }) {
  return (
    <div>
      <dt className="text-sm text-muted">{label}</dt>
      <dd className="text-lg font-semibold">{children}</dd>
    </div>
  );
}

/** The account's figures as a description list. */
function Figures({ account }: { readonly account: AccountDetail }) {
  return (
    <dl className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
      <Figure label="Equity (USDC)">{formatUsdc(account.equity)}</Figure>
      <Figure label="Balance (USDC)">{formatUsdc(account.balance)}</Figure>
      <Figure label="Realised P&L (USDC)"><Pnl value={account.realisedPnl} /></Figure>
      <Figure label="Unrealised P&L (USDC)"><Pnl value={account.unrealisedPnl} /></Figure>
      <Figure label="Margin level">{account.marginLevel === null ? "—" : formatRatioPercent(account.marginLevel)}</Figure>
      <Figure label="Open positions">{account.openPositions}</Figure>
      <Figure label="MONKEY earned">{formatDecimal(account.monkeyEarned, 2)}</Figure>
      <Figure label="MONKEY claimed">{formatDecimal(account.monkeyClaimed, 2)}</Figure>
    </dl>
  );
}

/** The page of the account at `address`. */
export function AccountPage({ address }: { readonly address: string }) {
  const { live } = useServices();
  const { details, detailFailures, connection } = useLive();
  useEffect(() => live.track(address), [live, address]);
  const account = details.get(address);
  const failure = detailFailures.get(address);

  return (
    <>
      <p className="mb-2"><Link to="/">All accounts</Link></p>
      <PageTitle>{account?.name ?? "Account"}</PageTitle>
      {account === undefined ? (
        <p className={failure === undefined ? "text-muted" : "text-loss"}>
          {failure === undefined ? "Loading account…" : `This account could not be read: ${failure}`}
        </p>
      ) : (
        <div className={`flex flex-col gap-6 ${connection === "live" ? "" : "opacity-70"}`}>
          <p className="break-all text-sm text-muted">
            {account.address} · {account.state}
            {account.stateReason === null ? "" : ` (${account.stateReason})`}
          </p>
          <AccountControls account={account} />
          <Figures account={account} />
          <section aria-labelledby="curve"><h2 id="curve" className="mb-2 text-xl font-semibold">Equity</h2><EquityCurve points={account.curve} /></section>
          <section aria-labelledby="strategies">
            <h2 id="strategies" className="mb-2 text-xl font-semibold">Strategies</h2>
            <div className="grid gap-3 md:grid-cols-2">
              {account.strategies.map((strategy) => (
                <StrategyCard key={strategy.id} address={account.address} strategy={strategy} />
              ))}
            </div>
          </section>
          <section aria-labelledby="positions"><h2 id="positions" className="mb-2 text-xl font-semibold">Positions</h2><PositionsTable positions={account.positions} /></section>
          <section aria-labelledby="orders"><h2 id="orders" className="mb-2 text-xl font-semibold">Orders</h2><OrdersTable orders={account.orders} /></section>
          <section aria-labelledby="fills"><h2 id="fills" className="mb-2 text-xl font-semibold">Fills</h2><FillsTable fills={account.fills} /></section>
        </div>
      )}
    </>
  );
}
