/** One account: figures, controls, equity curve, strategies, positions, orders and fills. */
import { useEffect, type ReactNode } from "react";

import type { Figures } from "@/api/types";
import { AccountControls } from "@/components/AccountControls";
import { useLive, useServices } from "@/components/app-context";
import { DayStopNote } from "@/components/DayStopNote";
import { EquityCurve } from "@/components/EquityCurve";
import { Link } from "@/components/Link";
import { PageTitle } from "@/components/PageTitle";
import { Pnl } from "@/components/Pnl";
import { FillsTable, OrdersTable, PositionsTable } from "@/components/PositionsTable";
import { StrategyCard } from "@/components/StrategyCard";
import { keyOf } from "@/api/live-state";
import { formatBpsPercent, formatMonkey, formatUsdc, shortAddress } from "@/lib/format";
import { symbolsOf } from "@/lib/markets";

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
function FigureList({ figures }: { readonly figures: Figures }) {
  const { monkey } = figures;
  return (
    <dl className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
      <Figure label="Equity (USDC)">{formatUsdc(figures.equity)}</Figure>
      <Figure label="Balance (USDC)">{formatUsdc(figures.balance)}</Figure>
      <Figure label="Realised P&L (USDC)"><Pnl value={figures.realisedPnl} /></Figure>
      <Figure label="Unrealised P&L (USDC)"><Pnl value={figures.unrealisedPnl} /></Figure>
      <Figure label="Margin level">{figures.marginLevelBps === null ? "—" : formatBpsPercent(figures.marginLevelBps)}</Figure>
      <Figure label="Margin required (USDC)">{formatUsdc(figures.margin)}</Figure>
      <Figure label="MONKEY earned">{formatMonkey(monkey.newestEntitled)}</Figure>
      <Figure label="MONKEY entitled to date">{formatMonkey(monkey.entitled)}</Figure>
      <Figure label="MONKEY claimed">{monkey.claimed === null ? "Not read yet" : formatMonkey(monkey.claimed)}</Figure>
    </dl>
  );
}

/** A section with its heading. */
function Section({ id, title, children }: { readonly id: string; readonly title: string; readonly children: ReactNode }) {
  return (
    <section aria-labelledby={id}>
      <h2 id={id} className="mb-2 text-xl font-semibold">{title}</h2>
      {children}
    </section>
  );
}

/** What a table says before the bot has read the account: not that it is empty. */
function Unread() {
  return <p className="text-muted">Waiting for the bot to read this account.</p>;
}

/** The page of the account at `address`. */
export function AccountPage({ address }: { readonly address: string }) {
  const { live } = useServices();
  const { accounts, curves, fills, fillFailures, curveFailures, loaded, connection } = useLive();
  useEffect(() => live.track(address), [live, address]);
  const key = keyOf(address);
  const account = accounts.find((candidate) => candidate.address !== null && keyOf(candidate.address) === key);
  const accountFills = fills.get(key) ?? [];
  const failure = fillFailures.get(key);

  return (
    <>
      <p className="mb-2"><Link to="/">All accounts</Link></p>
      <PageTitle>{account === undefined ? "Account" : shortAddress(address)}</PageTitle>
      {account === undefined ? (
        <p className="text-muted">{loaded ? "The bot has no such account." : "Loading account…"}</p>
      ) : (
        <div className={`flex flex-col gap-6 ${connection === "live" ? "" : "opacity-70"}`}>
          <p className="break-all text-sm text-muted">
            {address} · {account.state}
            {account.halt === null ? "" : ` (${account.halt})`}
          </p>
          {account.dayStop === null ? null : <p><DayStopNote dayStop={account.dayStop} /></p>}
          {account.error === null ? null : <p className="text-loss">{`The last read of the node failed: ${account.error}`}</p>}
          <AccountControls account={account} address={address} />
          {account.figures === null ? <p className="text-muted">The bot has not read this account's figures yet.</p> : <FigureList figures={account.figures} />}
          <Section id="curve" title="Equity">
            {curveFailures.get(key) === undefined ? null : (
              <p className="mb-2 text-loss">{`The whole day's curve could not be read: ${curveFailures.get(key) ?? ""}`}</p>
            )}
            <EquityCurve points={curves.get(key) ?? []} />
          </Section>
          <Section id="strategies" title="Strategies">
            <div className="grid gap-3 md:grid-cols-2">
              {account.strategies.map((strategy) => (
                <StrategyCard key={strategy.id} address={address} strategy={strategy} />
              ))}
            </div>
          </Section>
          <Section id="positions" title="Positions">
            {account.figures === null ? <Unread /> : <PositionsTable positions={account.figures.positions} />}
          </Section>
          <Section id="orders" title="Orders">
            {account.figures === null ? (
              <Unread />
            ) : (
              <OrdersTable orders={account.figures.orders} symbols={symbolsOf(account.figures.positions, accountFills)} />
            )}
          </Section>
          <Section id="fills" title="Fills">
            {failure === undefined ? null : <p className="mb-2 text-loss">{`Fills could not be read: ${failure}`}</p>}
            <FillsTable fills={accountFills} />
          </Section>
        </div>
      )}
    </>
  );
}
