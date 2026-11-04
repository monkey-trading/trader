/** The account's open positions, resting orders and recent fills as tables. */
import type { Fill, Order, Position } from "@/api/types";
import { signOf } from "@/lib/decimal";
import { formatDecimal, formatTime, formatTrimmed } from "@/lib/format";

import { CELL, DataTable } from "./DataTable";
import { Pnl } from "./Pnl";

/** Text for a missing price. */
const NONE = "—";

/** Long or short, by the sign of the quantity. */
function sideOf(qty: string): string {
  return signOf(qty) < 0 ? "Short" : "Long";
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
          <th scope="row" className={CELL}>{position.market}</th>
          <td className={CELL}>{sideOf(position.qty)}</td>
          <td className={`${CELL} text-right`}>{formatTrimmed(position.qty, 6)}</td>
          <td className={`${CELL} text-right`}>{formatTrimmed(position.entry, 4)}</td>
          <td className={`${CELL} text-right`}>{position.mark === null ? NONE : formatTrimmed(position.mark, 4)}</td>
          <td className={`${CELL} text-right`}><Pnl value={position.unrealisedPnl} /></td>
          <td className={`${CELL} text-right`}>{position.closeout === null ? NONE : formatTrimmed(position.closeout, 4)}</td>
        </tr>
      ))}
    </DataTable>
  );
}

/** Resting orders. */
export function OrdersTable({ orders }: { readonly orders: readonly Order[] }) {
  if (orders.length === 0) {
    return <p className="text-muted">No resting orders.</p>;
  }
  return (
    <DataTable label="Resting orders" head={["Order", "Market", "Side", "Quantity", "Limit price", "Reduce only"]} numeric={[3, 4]}>
      {orders.map((order) => (
        <tr key={order.orderId} className="border-t border-line">
          <th scope="row" className={CELL}>{order.orderId}</th>
          <td className={CELL}>{order.market}</td>
          <td className={CELL}>{order.side === "buy" ? "Buy" : "Sell"}</td>
          <td className={`${CELL} text-right`}>{formatTrimmed(order.qty, 6)}</td>
          <td className={`${CELL} text-right`}>{order.price === null ? NONE : formatTrimmed(order.price, 4)}</td>
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
      head={["Time (UTC)", "Market", "Side", "Quantity", "Price", "Fee (USDC)", "Realised P&L (USDC)"]}
      numeric={[3, 4, 5, 6]}
    >
      {fills.map((fill) => (
        <tr key={fill.id} className="border-t border-line">
          <th scope="row" className={CELL}>{formatTime(fill.time)}</th>
          <td className={CELL}>{fill.market}</td>
          <td className={CELL}>{fill.side === "buy" ? "Buy" : "Sell"}</td>
          <td className={`${CELL} text-right`}>{formatTrimmed(fill.qty, 6)}</td>
          <td className={`${CELL} text-right`}>{formatTrimmed(fill.price, 4)}</td>
          <td className={`${CELL} text-right`}>{formatDecimal(fill.fee, 4)}</td>
          <td className={`${CELL} text-right`}><Pnl value={fill.realisedPnl} /></td>
        </tr>
      ))}
    </DataTable>
  );
}
