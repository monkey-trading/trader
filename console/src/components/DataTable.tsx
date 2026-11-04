/** A table that scrolls inside its own labelled, focusable container so the page never scrolls sideways. */
import type { ReactNode } from "react";

interface DataTableProps {
  readonly label: string;
  readonly head: readonly string[];
  /** Columns that hold numbers and align right. */
  readonly numeric?: readonly number[];
  readonly children: ReactNode;
}

/** The shared cell style. */
export const CELL = "px-2 py-1 whitespace-nowrap";

/** A real `<table>` with column headers. */
export function DataTable({ label, head, numeric = [], children }: DataTableProps) {
  return (
    <div role="region" aria-label={label} className="relative overflow-x-auto rounded border border-line focus-visible:outline-2 focus-visible:outline-accent">
      <table className="w-full border-collapse text-left text-sm">
        <caption className="sr-only">{label}</caption>
        <thead className="bg-surface">
          <tr>
            {head.map((title, index) => (
              <th key={title} scope="col" className={`${CELL} font-semibold ${numeric.includes(index) ? "text-right" : ""}`}>
                {title}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>{children}</tbody>
      </table>
    </div>
  );
}
