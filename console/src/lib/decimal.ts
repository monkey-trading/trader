/** Exact decimal arithmetic on the strings the bot serves, as `bigint`s scaled by 10^18. */

/** Fractional digits kept: the engine's amounts have nine, MONKEY amounts up to eighteen. */
export const DECIMAL_DIGITS = 18;

const SCALE = 10n ** BigInt(DECIMAL_DIGITS);
const DECIMAL = /^-?\d{1,30}(?:\.\d{1,18})?$/;

/** Whether `text` is a plain decimal: optional minus, digits, at most eighteen fractional digits. */
export function isDecimal(text: string): boolean {
  return DECIMAL.test(text);
}

/**
 * Parses a decimal string into units of 10^-18.
 * Error cases:
 * - `text` is not a plain decimal (check with `isDecimal` first on untrusted input)
 */
export function parseUnits(text: string): bigint {
  if (!isDecimal(text)) {
    throw new Error(`not a decimal: ${JSON.stringify(text.slice(0, 40))}`);
  }
  const negative = text.startsWith("-");
  const [whole = "0", fraction = ""] = (negative ? text.slice(1) : text).split(".");
  const units = BigInt(whole) * SCALE + BigInt(fraction.padEnd(DECIMAL_DIGITS, "0"));
  return negative ? -units : units;
}

/** The sum of decimal strings, in units; a string that is not a decimal makes the sum `null`. */
export function sumUnits(values: readonly string[]): bigint | null {
  let total = 0n;
  for (const value of values) {
    if (!isDecimal(value)) {
      return null;
    }
    total += parseUnits(value);
  }
  return total;
}

/** Compares two decimal strings: negative, zero or positive as `a` is below, equal to or above `b`. */
export function compareDecimals(a: string, b: string): number {
  const difference = parseUnits(a) - parseUnits(b);
  return difference === 0n ? 0 : difference < 0n ? -1 : 1;
}

/** The sign of a decimal string: -1, 0 or 1. */
export function signOf(text: string): -1 | 0 | 1 {
  const units = parseUnits(text);
  return units === 0n ? 0 : units < 0n ? -1 : 1;
}

/**
 * Renders units as a decimal string truncated toward zero to `places` fractional digits.
 * Error cases:
 * - `places` is not an integer from 0 to 18
 */
export function unitsToText(units: bigint, places: number): string {
  if (!Number.isInteger(places) || places < 0 || places > DECIMAL_DIGITS) {
    throw new Error(`places out of range: ${String(places)}`);
  }
  const absolute = units < 0n ? -units : units;
  const step = 10n ** BigInt(DECIMAL_DIGITS - places);
  const kept = absolute / step;
  const digits = kept.toString().padStart(places + 1, "0");
  const whole = digits.slice(0, digits.length - places);
  const fraction = digits.slice(digits.length - places);
  const negative = units < 0n && kept !== 0n;
  return `${negative ? "-" : ""}${whole}${places > 0 ? `.${fraction}` : ""}`;
}
