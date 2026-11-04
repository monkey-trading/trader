/** A control that changes server state: pending, failed and done in place, never twice at once. */
import { useRef, useState } from "react";

import type { Outcome } from "@/api/client";
import type { AccountRead } from "@/api/types";

import { useServices } from "./app-context";

/** What a control shows and does. */
export interface Action {
  readonly pending: boolean;
  /** Why the last attempt failed, until the next one starts. */
  readonly error: string | null;
  /** Starts `call` unless one is already in flight. */
  readonly run: (call: () => Promise<Outcome<AccountRead>>) => void;
}

/** State for a control whose calls return the changed account: success updates the book, 401 ends the session. */
export function useAction(): Action {
  const { live, session } = useServices();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Two clicks in one tick both see pending=false in state; the ref closes that gap.
  const inFlight = useRef(false);

  const run = (call: () => Promise<Outcome<AccountRead>>) => {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    setPending(true);
    setError(null);
    void call().then((outcome) => {
      inFlight.current = false;
      setPending(false);
      switch (outcome.status) {
        case "ok":
          live.accept(outcome.value);
          break;
        case "unauthenticated":
          session.expire();
          break;
        case "refused":
        case "unavailable":
          setError(outcome.message);
          break;
      }
    });
  };
  return { pending, error, run };
}
