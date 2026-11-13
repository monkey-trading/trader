/** A control that changes server state: pending, failed and done in place, never twice at once. */
import { useRef, useState } from "react";

import type { Outcome, Warned } from "@/api/client";
import { useServices } from "./app-context";

/** What a control shows and does. */
export interface Action {
  readonly pending: boolean;
  /** Why the last attempt failed, until the next one starts. */
  readonly error: string | null;
  /** What the bot said about an attempt that worked, until the next one starts. */
  readonly warning: string | null;
  /** Whether the last attempt worked, until the next one starts. */
  readonly done: boolean;
  /** Starts `call` unless one is already in flight. */
  readonly run: (call: () => Promise<Outcome<Warned | null>>) => void;
}

/** State for a control that changes `address`: success reads the account again, 401 ends the session. */
export function useAction(address: string): Action {
  const { live, session } = useServices();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [warning, setWarning] = useState<string | null>(null);
  const [done, setDone] = useState(false);
  // Two clicks in one tick both see pending=false in state; the ref closes that gap.
  const inFlight = useRef(false);

  const run = (call: () => Promise<Outcome<Warned | null>>) => {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    const generation = session.generation;
    setPending(true);
    setError(null);
    setWarning(null);
    setDone(false);
    void call().then((outcome) => {
      inFlight.current = false;
      setPending(false);
      switch (outcome.status) {
        case "ok":
          setWarning(outcome.value?.warning ?? null);
          setDone(true);
          void live.refresh(address);
          break;
        case "unauthenticated":
          // A 401 to a request from an earlier session says nothing of this one.
          session.expire(generation);
          break;
        case "refused":
        case "unavailable":
          setError(outcome.message);
          break;
      }
    });
  };
  return { pending, error, warning, done, run };
}
