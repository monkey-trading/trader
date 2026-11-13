/** A signed profit-and-loss figure: the sign carries the meaning, the colour only helps. */
import { signOf } from "@/lib/decimal";
import { formatSigned, UNAVAILABLE } from "@/lib/format";

/** The bot's amount, formatted with its sign and coloured by it. */
export function Pnl({ value }: { readonly value: string }) {
  const text = formatSigned(value);
  const tone = text === UNAVAILABLE ? "" : signOf(value) > 0 ? "text-gain" : signOf(value) < 0 ? "text-loss" : "";
  return <span className={tone}>{text}</span>;
}
