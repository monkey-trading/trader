/** Display formatting for the bot's decimal strings, independent of the user's locale. */
import { baseUnitsToDecimal, isDecimal, parseUnits, unitsToText } from "./decimal";

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

/** A margin level in basis points as a percentage with two places: `202500` reads `2,025.00%`. */
export function formatBpsPercent(bps: number): string {
  if (!Number.isSafeInteger(bps)) {
    return UNAVAILABLE;
  }
  const absolute = Math.abs(bps);
  const whole = Math.trunc(absolute / 100);
  const hundredths = String(absolute % 100).padStart(2, "0");
  return `${bps < 0 ? "-" : ""}${group(String(whole))}.${hundredths}%`;
}

/** An amount of MONKEY given in base units (18 decimals) as whole MONKEY with two places, truncated. */
export function formatMonkey(baseUnits: string): string {
  return /^\d{1,40}$/.test(baseUnits) ? formatDecimal(baseUnitsToDecimal(baseUnits), 2) : UNAVAILABLE;
}

/** An address as its first six and last four characters. */
export function shortAddress(address: string): string {
  return address.length > 12 ? `${address.slice(0, 6)}…${address.slice(-4)}` : address;
}

/** A Unix time in milliseconds as `YYYY-MM-DD HH:MM:SS` UTC. */
export function formatTime(millis: number): string {
  return new Date(millis).toISOString().slice(0, 19).replace("T", " ");
}

/** A Unix time in milliseconds as `YYYY-MM-DD HH:MM` at `offsetMinutes` from UTC. */
function formatMinute(millis: number, offsetMinutes: number): string {
  return new Date(millis + offsetMinutes * 60_000).toISOString().slice(0, 16).replace("T", " ");
}

/** An offset from UTC as `UTC+01:00`. */
function formatOffset(offsetMinutes: number): string {
  const absolute = Math.abs(offsetMinutes);
  const hours = String(Math.trunc(absolute / 60)).padStart(2, "0");
  return `UTC${offsetMinutes < 0 ? "-" : "+"}${hours}:${String(absolute % 60).padStart(2, "0")}`;
}

/**
 * The note for an account the daily loss limit has stopped: the loss and when trading resumes, in local time at
 * `offsetMinutes` from UTC (the browser's offset at that time) and in UTC.
 */
export function formatDayStop(dayStop: { readonly untilMs: number; readonly loss: string }, offsetMinutes: number): string {
  const local = `${formatMinute(dayStop.untilMs, offsetMinutes)} local time (${formatOffset(offsetMinutes)})`;
  return `Stopped for the day: daily loss ${formatUsdc(dayStop.loss)} USD, trading resumes at ${local}, ${formatMinute(dayStop.untilMs, 0)} UTC.`;
}
