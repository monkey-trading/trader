/** Start, stop and flatten for one account. Flatten asks for a second press. */
import { useRef, useState } from "react";

import type { AccountSummary } from "@/api/types";

import { ActionButton, ActionError, BUTTON_CLASS } from "./ActionButton";
import { useServices } from "./app-context";
import { useAction } from "./use-action";

/** Closes every position and cancels every order of the account, after a confirming press. */
function FlattenControl({ address }: { readonly address: string }) {
  const { api } = useServices();
  const [confirming, setConfirming] = useState(false);
  const action = useAction();
  const ask = useRef<HTMLButtonElement>(null);
  const confirm = useRef<HTMLButtonElement>(null);

  if (!confirming) {
    return (
      <div>
        <button
          ref={ask}
          type="button"
          className={BUTTON_CLASS}
          disabled={action.pending}
          onClick={() => {
            setConfirming(true);
            // The confirm button mounts on the next render; focus follows it.
            requestAnimationFrame(() => confirm.current?.focus());
          }}
        >
          {action.pending ? "Flattening…" : "Flatten account"}
        </button>
        <ActionError error={action.error} />
      </div>
    );
  }
  return (
    <div role="group" aria-label="Confirm flatten">
      <p className="mb-1 text-sm">Close all positions and cancel all orders?</p>
      <div className="flex gap-2">
        <button
          ref={confirm}
          type="button"
          className={BUTTON_CLASS}
          disabled={action.pending}
          onClick={() => {
            action.run(() => api.flatten(address));
            setConfirming(false);
          }}
        >
          Confirm flatten
        </button>
        <button
          type="button"
          className={BUTTON_CLASS}
          onClick={() => {
            setConfirming(false);
            requestAnimationFrame(() => ask.current?.focus());
          }}
        >
          Cancel
        </button>
      </div>
    </div>
  );
}

/** The account's controls. */
export function AccountControls({ account }: { readonly account: AccountSummary }) {
  const { api } = useServices();
  const running = account.state === "running";
  const toggle = useAction();
  return (
    <div className="flex flex-wrap items-start gap-4">
      <ActionButton
        label={running ? "Stop account" : "Start account"}
        pendingLabel={running ? "Stopping…" : "Starting…"}
        action={toggle}
        call={() => api.setAccountRunning(account.address, !running)}
      />
      <FlattenControl address={account.address} />
    </div>
  );
}
