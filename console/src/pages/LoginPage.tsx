/** The login page: one password field, refusals shown at the field. */
import { useId, useRef, useState, type SubmitEvent } from "react";

import { BUTTON_CLASS } from "@/components/ActionButton";
import { useServices } from "@/components/app-context";
import { PageTitle } from "@/components/PageTitle";

/** Longest password the form takes; the bot's own limit applies as well. */
const MAX_PASSWORD = 200;

/** Asks for the password the bot printed on its first run. */
export function LoginPage() {
  const { session } = useServices();
  const [password, setPassword] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef(false);
  const fieldId = useId();
  const errorId = useId();

  const onSubmit = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (inFlight.current || password === "") {
      return;
    }
    inFlight.current = true;
    setPending(true);
    setError(null);
    void session.login(password).then((result) => {
      inFlight.current = false;
      setPending(false);
      if (result.status === "refused") {
        setError(result.message);
      }
    });
  };

  return (
    <>
      <PageTitle focus={false}>Sign in</PageTitle>
      <form onSubmit={onSubmit} className="flex max-w-sm flex-col gap-3">
        <label htmlFor={fieldId} className="font-medium">
          Password
        </label>
        <input
          id={fieldId}
          type="password"
          autoComplete="current-password"
          maxLength={MAX_PASSWORD}
          value={password}
          onChange={(event) => {
            setPassword(event.target.value);
          }}
          aria-invalid={error !== null}
          aria-describedby={errorId}
          className="min-h-9 rounded border border-line bg-surface px-2 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
        />
        <p id={errorId} aria-live="polite" className="min-h-5 text-sm text-loss">
          {error ?? ""}
        </p>
        <button type="submit" disabled={pending} className={BUTTON_CLASS}>
          {pending ? "Signing in…" : "Sign in"}
        </button>
      </form>
    </>
  );
}
