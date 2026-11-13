/** A funding action: pending, failed and started in place, never twice at once. */
import { useRef, useState } from "react";

import type { Outcome } from "@/api/client";
import type { FundingJob } from "@/api/funding-types";
import { useServices } from "./app-context";

/** What a funding control shows and does. */
export interface FundingAction {
  readonly pending: boolean;
  /** Why the last request was refused, until the next starts. */
  readonly error: string | null;
  /** How many jobs the last request started, until the next starts; null before one has. */
  readonly started: number | null;
  readonly run: (call: () => Promise<Outcome<readonly FundingJob[]>>) => void;
}

/** State for a control whose request starts jobs; `reload` reads the jobs at once when it did. */
export function useFundingAction(reload: () => void): FundingAction {
  const { session } = useServices();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [started, setStarted] = useState<number | null>(null);
  const inFlight = useRef(false);
  const run = (call: () => Promise<Outcome<readonly FundingJob[]>>) => {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    const generation = session.generation;
    setPending(true);
    setError(null);
    setStarted(null);
    void call().then((outcome) => {
      inFlight.current = false;
      setPending(false);
      switch (outcome.status) {
        case "ok":
          setStarted(outcome.value.length);
          reload();
          break;
        case "unauthenticated":
          session.expire(generation);
          break;
        case "refused":
          setError(outcome.message);
          break;
        case "unavailable":
          // The request may have started jobs before the answer was lost.
          setError(outcome.message);
          reload();
          break;
      }
    });
  };
  return { pending, error, started, run };
}
