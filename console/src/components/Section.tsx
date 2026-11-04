/** A titled section of a page with its heading wired to the region. */
import { useId, type ReactNode } from "react";

interface SectionProps {
  readonly title: string;
  readonly children: ReactNode;
}

/** A `<section>` named by its `h2`. */
export function Section({ title, children }: SectionProps) {
  const id = useId();
  return (
    <section aria-labelledby={id} className="rounded border border-line bg-surface p-3">
      <h2 id={id} className="mb-2 text-xl font-semibold">{title}</h2>
      {children}
    </section>
  );
}
