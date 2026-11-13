/**
 * The funding read, kept current by itself: quickly while a job has not ended or an account is still
 * being read, slowly otherwise, and not at all while the page is hidden, so the bot stops reading the
 * accounts for it.
 */
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

/** Reads the funding on mount and then on a timer while the page is shown, until it goes. A 401 ends the session. */
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
    let unlisten = () => {};
    /** Resolves once the page is shown, or the effect is cleaned up. */
    const shown = () =>
      new Promise<void>((resolve) => {
        const check = () => {
          if (!document.hidden) {
            resolve();
          }
        };
        document.addEventListener("visibilitychange", check);
        unlisten = () => {
          document.removeEventListener("visibilitychange", check);
        };
        wake.current = resolve;
      });
    const loop = async () => {
      while (live()) {
        const generation = session.generation;
        const outcome = await api.funding();
        if (!live()) {
          return;
        }
        let fast = false;
        switch (outcome.status) {
          case "ok":
            setFunding(outcome.value);
            setError(null);
            // A job has not ended, or the bot is still reading the accounts: ask again soon, to show them as they come.
            fast =
              outcome.value.enabled &&
              (outcome.value.jobs.some((job) => !ended(job)) || outcome.value.accounts.some((account) => account.reading));
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
          timer = setTimeout(resolve, fast ? BUSY_MS : IDLE_MS);
        });
        while (live() && document.hidden) {
          await shown();
          unlisten();
        }
        wake.current = null;
      }
    };
    void loop();
    return () => {
      life.active = false;
      clearTimeout(timer);
      unlisten();
      wake.current?.();
    };
  }, [api, session]);
  const reload = useCallback(() => {
    wake.current?.();
  }, []);
  return { funding, error, reload };
}
