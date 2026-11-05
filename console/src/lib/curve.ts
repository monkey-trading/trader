/** Layout of the equity curve. Floats here place pixels; no figure shown as an amount comes from them. */
import { parseUnits, unitsToText } from "./decimal";

/** The drawing's size in SVG units. */
export const CURVE_WIDTH = 600;
export const CURVE_HEIGHT = 160;

/** A point of the curve as the bot serves it. */
export interface RawPoint {
  readonly timeMs: number;
  readonly equity: string;
}

/** The curve's range as decimal strings, for its text alternative. */
export interface CurveRange {
  readonly low: string;
  readonly high: string;
  readonly latest: string;
}

/** The lowest, highest and latest equity, or null for an empty curve. */
export function curveRange(points: readonly RawPoint[]): CurveRange | null {
  const last = points.at(-1);
  if (last === undefined) {
    return null;
  }
  const values = points.map((point) => parseUnits(point.equity));
  const low = values.reduce((a, b) => (b < a ? b : a));
  const high = values.reduce((a, b) => (b > a ? b : a));
  return { low: unitsToText(low, 2), high: unitsToText(high, 2), latest: unitsToText(parseUnits(last.equity), 2) };
}

/** The SVG `points` of the curve scaled into the drawing, or an empty string for fewer than two points. */
export function curvePath(points: readonly RawPoint[]): string {
  const first = points[0];
  const last = points.at(-1);
  if (first === undefined || last === undefined || points.length < 2) {
    return "";
  }
  const values = points.map((point) => parseUnits(point.equity));
  const low = values.reduce((a, b) => (b < a ? b : a));
  const high = values.reduce((a, b) => (b > a ? b : a));
  const span = Number(high - low);
  const duration = last.timeMs - first.timeMs;
  return points
    .map((point, index) => {
      const x = duration > 0 ? ((point.timeMs - first.timeMs) / duration) * CURVE_WIDTH : (index / (points.length - 1)) * CURVE_WIDTH;
      const y = span > 0 ? CURVE_HEIGHT - (Number((values[index] ?? low) - low) / span) * CURVE_HEIGHT : CURVE_HEIGHT / 2;
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
}
