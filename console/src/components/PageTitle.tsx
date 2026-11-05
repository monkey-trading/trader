/** The page's one h1, which takes focus when its page appears so a route change is announced. */
import { useEffect, useRef } from "react";

interface PageTitleProps {
  readonly children: string;
  /** False on the login page, so the skip link stays first in the tab order of a fresh load. */
  readonly focus?: boolean;
}

/** The page heading. */
export function PageTitle({ children, focus = true }: PageTitleProps) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    if (focus) {
      heading.current?.focus();
    }
  }, [focus]);
  return (
    <h1 ref={heading} tabIndex={-1} className="mb-4 text-2xl font-semibold outline-none">
      {children}
    </h1>
  );
}
