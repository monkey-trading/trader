/** Display formatting for the bot's decimal strings, independent of the user's locale. */
import { isDecimal, parseUnits, unitsToText } from "./decimal";

/** What stands where a figure is missing or cannot be read. */
export const UNAVAILABLE = "unavailable";

/** Inserts a comma between thousands of a digit string. */
function group(whole: string): string {
  return whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

/** Groups the whole part of a rendered decimal. */
function grouped(text: string): string {
  const negative = text.startsWith("-");
  const [whole = "0", fraction] = (negative ? text.slice(1) : text).split(".");
  return `${negative ? "-" : ""}${group(whole)}${fraction === undefined ? "" : `.${fraction}`}`;
}

/** A decimal string with thousands separators, truncated toward zero to `places` fractional digits. */
export function formatDecimal(value: string, places: number): string {
  return isDecimal(value) ? grouped(unitsToText(parseUnits(value), places)) : UNAVAILABLE;
}

/** An amount in USDC with two places, such as `1,234.50`. */
export function formatUsdc(value: string): string {
  return formatDecimal(value, 2);
}

/** A signed amount: `+` for a gain, `-` for a loss, nothing for zero. */
export function formatSigned(value: string, places = 2): string {
  const text = formatDecimal(value, places);
  if (text === UNAVAILABLE || text.startsWith("-")) {
    return text;
  }
  return Number(text.replace(/[,.]/g, "")) === 0 ? text : `+${text}`;
}

/** A quantity or price: up to `places` fractional digits, trailing zeros dropped. */
export function formatTrimmed(value: string, places: number): string {
  const text = formatDecimal(value, places);
  return text.includes(".") ? text.replace(/\.?0+$/, "") : text;
}

/** A ratio as a percentage with two places: `2.5` reads `250.00%`. */
export function formatRatioPercent(value: string): string {
  return isDecimal(value) ? `${grouped(unitsToText(parseUnits(value) * 100n, 2))}%` : UNAVAILABLE;
}

/** An address as its first six and last four characters. */
export function shortAddress(address: string): string {
  return address.length > 12 ? `${address.slice(0, 6)}…${address.slice(-4)}` : address;
}

/** A Unix time in milliseconds as `YYYY-MM-DD HH:MM:SS` UTC. */
export function formatTime(millis: number): string {
  return new Date(millis).toISOString().slice(0, 19).replace("T", " ");
}
