/** A link inside the console that changes the page without a load. */
import type { MouseEvent, ReactNode } from "react";

import { navigate } from "./use-route";

interface LinkProps {
  readonly to: string;
  readonly children: ReactNode;
}

/** An `<a href>`: a plain click navigates in place, a modified click or middle click does what a browser link does. */
export function Link({ to, children }: LinkProps) {
  const onClick = (event: MouseEvent<HTMLAnchorElement>) => {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      return;
    }
    event.preventDefault();
    navigate(to);
  };
  return (
    <a href={to} onClick={onClick} className="text-accent underline underline-offset-2 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent">
      {children}
    </a>
  );
}
