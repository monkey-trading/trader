/** A control that changes server state: pending, failed and done in place, never twice at once. */
import { useRef, useState } from "react";

import type { Outcome } from "@/api/client";
import { useServices } from "./app-context";

/** What a control shows and does. */
export interface Action {
  readonly pending: boolean;
  /** Why the last attempt failed, until the next one starts. */
  readonly error: string | null;
  /** Starts `call` unless one is already in flight. */
  readonly run: (call: () => Promise<Outcome<null>>) => void;
}

/** State for a control that changes `address`: success reads the account again, 401 ends the session. */
export function useAction(address: string): Action {
  const { live, session } = useServices();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Two clicks in one tick both see pending=false in state; the ref closes that gap.
  const inFlight = useRef(false);

  const run = (call: () => Promise<Outcome<null>>) => {
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
          void live.refresh(address);
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
