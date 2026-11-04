/** A funding control: pending and failed in place, never submitted twice at once. */
import { useRef, useState } from "react";

import type { Outcome } from "@/api/client";
import type { FundingRead } from "@/api/funding-types";

import { useServices } from "./app-context";

/** What a funding control shows and does. */
export interface FundingAction {
  readonly pending: boolean;
  /** Why the last attempt failed, until the next one starts. */
  readonly error: string | null;
  /** Starts `call` unless one is in flight; `onDone` runs once the bot accepts it. */
  readonly run: (call: () => Promise<Outcome<FundingRead>>, onDone?: () => void) => void;
}

/** State for a control whose calls return the funding read: success updates the book, 401 ends the session. */
export function useFundingAction(): FundingAction {
  const { funding, session } = useServices();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Two clicks in one tick both see pending=false in state; the ref closes that gap.
  const inFlight = useRef(false);

  const run: FundingAction["run"] = (call, onDone) => {
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
          funding.accept(outcome.value);
          onDone?.();
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
