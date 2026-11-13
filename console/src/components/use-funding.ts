/** The funding read, kept current by itself: quickly while a job has not ended, slowly otherwise. */
import { useCallback, useEffect, useRef, useState } from "react";

import type { Funding } from "@/api/funding-types";
import { ended } from "@/api/funding-types";
import { useServices } from "./app-context";

/** How long after a read the next one is made while a job has not ended, and when none has. */
export const BUSY_MS = 1500;
export const IDLE_MS = 10_000;

/** The state of the funding read. */
export interface FundingRead {
  /** The last answer; null until the first. */
  readonly funding: Funding | null;
  /** Why the last read failed, until the next one works. */
  readonly error: string | null;
  /** Reads again now. */
  readonly reload: () => void;
}

/** Reads the funding on mount and then on a timer, until the page goes. A 401 ends the session. */
export function useFunding(): FundingRead {
  const { api, session } = useServices();
  const [funding, setFunding] = useState<Funding | null>(null);
  const [error, setError] = useState<string | null>(null);
  const wake = useRef<(() => void) | null>(null);
  useEffect(() => {
    const life = { active: true };
    // A call, since a flag read twice either side of an await would be narrowed to its first value.
    const live = () => life.active;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const loop = async () => {
      while (live()) {
        const generation = session.generation;
        const outcome = await api.funding();
        if (!live()) {
          return;
        }
        let wait = IDLE_MS;
        switch (outcome.status) {
          case "ok":
            setFunding(outcome.value);
            setError(null);
            if (outcome.value.enabled && outcome.value.jobs.some((job) => !ended(job))) {
              wait = BUSY_MS;
            }
            break;
          case "unauthenticated":
            session.expire(generation);
            return;
          case "refused":
          case "unavailable":
            setError(outcome.message);
            break;
        }
        await new Promise<void>((resolve) => {
          wake.current = resolve;
          timer = setTimeout(resolve, wait);
        });
        wake.current = null;
      }
    };
    void loop();
    return () => {
      life.active = false;
      clearTimeout(timer);
      wake.current?.();
    };
  }, [api, session]);
  const reload = useCallback(() => {
    wake.current?.();
  }, []);
  return { funding, error, reload };
}
