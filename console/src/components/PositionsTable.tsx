/** The account's open positions, resting orders and recent fills as tables. */
import type { Fill, Order, Position } from "@/api/types";
import { signOf } from "@/lib/decimal";
import { formatTime, formatTrimmed } from "@/lib/format";
import { marketName } from "@/lib/markets";

import { CELL, DataTable } from "./DataTable";
import { Pnl } from "./Pnl";

/** Text for a missing figure. */
const NONE = "—";

/** Long or short, by the sign of the quantity. */
function sideOf(qty: string): string {
  return signOf(qty) < 0 ? "Short" : "Long";
}

/** A price, or the placeholder when the bot gave none. */
function price(value: string | null): string {
  return value === null ? NONE : formatTrimmed(value, 4);
}

/** Open positions; a negative quantity is short. */
export function PositionsTable({ positions }: { readonly positions: readonly Position[] }) {
  if (positions.length === 0) {
    return <p className="text-muted">No open positions.</p>;
  }
  return (
    <DataTable
      label="Open positions"
      head={["Market", "Side", "Quantity", "Entry", "Mark", "Unrealised P&L (USDC)", "Close-out price"]}
      numeric={[2, 3, 4, 5, 6]}
    >
      {positions.map((position) => (
        <tr key={position.market} className="border-t border-line">
          <th scope="row" className={CELL}>{position.symbol ?? marketName(position.market, new Map())}</th>
          <td className={CELL}>{sideOf(position.qty)}</td>
          <td className={`${CELL} text-right`}>{formatTrimmed(position.qty, 6)}</td>
          <td className={`${CELL} text-right`}>{price(position.entry)}</td>
          <td className={`${CELL} text-right`}>{price(position.mark)}</td>
          <td className={`${CELL} text-right`}>{position.unrealisedPnl === null ? NONE : <Pnl value={position.unrealisedPnl} />}</td>
          <td className={`${CELL} text-right`}>{price(position.closeout)}</td>
        </tr>
      ))}
    </DataTable>
  );
}

/** What an order rests as: a limit price, or a market order's quoted price. */
function kindOf(order: Order): string {
  return order.kind.type === "limit"
    ? `Limit at ${formatTrimmed(order.kind.price, 4)}`
    : `Market, quoted ${formatTrimmed(order.kind.quoted, 4)}`;
}

/** Resting orders; `symbols` names their markets. */
export function OrdersTable({ orders, symbols }: { readonly orders: readonly Order[]; readonly symbols: ReadonlyMap<number, string> }) {
  if (orders.length === 0) {
    return <p className="text-muted">No resting orders.</p>;
  }
  return (
    <DataTable label="Resting orders" head={["Order", "Market", "Side", "Quantity", "Kind", "Reduce only"]} numeric={[3]}>
      {orders.map((order) => (
        <tr key={order.orderId} className="border-t border-line">
          <th scope="row" className={CELL}>{order.orderId}</th>
          <td className={CELL}>{marketName(order.market, symbols)}</td>
          <td className={CELL}>{order.side === "buy" ? "Buy" : "Sell"}</td>
          <td className={`${CELL} text-right`}>{formatTrimmed(order.qty, 6)}</td>
          <td className={CELL}>{kindOf(order)}</td>
          <td className={CELL}>{order.reduceOnly ? "Yes" : "No"}</td>
        </tr>
      ))}
    </DataTable>
  );
}

/** Recent fills, newest first. */
export function FillsTable({ fills }: { readonly fills: readonly Fill[] }) {
  if (fills.length === 0) {
    return <p className="text-muted">No fills yet.</p>;
  }
  return (
    <DataTable
      label="Recent fills"
      head={["Time (UTC)", "Market", "Side", "Quantity", "Price", "Realised P&L (USDC)", "Profit fee (USDC)"]}
      numeric={[3, 4, 5, 6]}
    >
      {fills.map((fill) => (
        <tr key={fill.cursor} className="border-t border-line">
          <th scope="row" className={CELL}>{formatTime(fill.time)}</th>
          <td className={CELL}>{fill.symbol ?? marketName(fill.market, new Map())}</td>
          <td className={CELL}>{fill.side === "buy" ? "Buy" : "Sell"}</td>
          <td className={`${CELL} text-right`}>{formatTrimmed(fill.qty, 6)}</td>
          <td className={`${CELL} text-right`}>{formatTrimmed(fill.price, 4)}</td>
          <td className={`${CELL} text-right`}><Pnl value={fill.realisedPnl} /></td>
          <td className={`${CELL} text-right`}>{formatTrimmed(fill.profitFee, 4)}</td>
        </tr>
      ))}
    </DataTable>
  );
}
