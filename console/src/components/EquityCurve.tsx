/** The equity curve as an SVG line with a text alternative of its latest values. */
import type { CurvePoint } from "@/api/types";
import { CURVE_HEIGHT, CURVE_WIDTH, curvePath, curveRange } from "@/lib/curve";
import { formatUsdc } from "@/lib/format";

/** Draws the curve; fewer than two points say so instead. */
export function EquityCurve({ points }: { readonly points: readonly CurvePoint[] }) {
  const range = curveRange(points);
  const path = curvePath(points);
  if (range === null || path === "") {
    return <p className="text-muted">The equity curve starts once the account has two readings.</p>;
  }
  const description = `Equity now ${formatUsdc(range.latest)} USDC; lowest ${formatUsdc(range.low)}, highest ${formatUsdc(range.high)} over ${String(points.length)} readings.`;
  return (
    <figure>
      <svg
        role="img"
        aria-label={description}
        viewBox={`0 0 ${String(CURVE_WIDTH)} ${String(CURVE_HEIGHT)}`}
        preserveAspectRatio="none"
        className="h-40 w-full rounded border border-line bg-surface"
      >
        <polyline points={path} fill="none" strokeWidth="2" vectorEffect="non-scaling-stroke" className="stroke-accent" />
      </svg>
      <figcaption className="mt-1 text-sm text-muted">{description}</figcaption>
    </figure>
  );
}
