/** Names for markets the bot reports by id. */

/** A market the bot may name by symbol. */
interface Named {
  readonly market: number;
  readonly symbol: string | null;
}

/** The symbols known for market ids, from whatever positions and fills name them. */
export function symbolsOf(...lists: readonly (readonly Named[])[]): ReadonlyMap<number, string> {
  const symbols = new Map<number, string>();
  for (const item of lists.flat()) {
    if (item.symbol !== null) {
      symbols.set(item.market, item.symbol);
    }
  }
  return symbols;
}

/** A market's symbol, or its id when none is known. */
export function marketName(market: number, symbols: ReadonlyMap<number, string>): string {
  return symbols.get(market) ?? `Market ${String(market)}`;
}
