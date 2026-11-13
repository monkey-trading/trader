/** Start, stop and flatten for one account. Flatten asks for a second press. */
import { useRef, useState, type RefObject } from "react";

import type { AccountView } from "@/api/types";

import { ActionButton, ActionError, BUTTON_CLASS } from "./ActionButton";
import { useServices } from "./app-context";
import { useAction, type Action } from "./use-action";

/**
 * Closes every position and cancels every order of the account, after a confirming press. Its action is the
 * parent's, which outlives the control: the account reads `flattening` while the request is out, and the answer
 * must show where it was asked for, whatever the account's state became meanwhile.
 */
function FlattenControl({
  address,
  action,
  status,
}: {
  readonly address: string;
  readonly action: Action;
  /** The status line that stays on the page, which focus moves to once the confirm button has gone. */
  readonly status: RefObject<HTMLParagraphElement | null>;
}) {
  const { api } = useServices();
  const [confirming, setConfirming] = useState(false);
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
          onClick={(event) => {
            // The second click of a double-click lands on the confirm button that the first brought up.
            if (event.detail > 1) {
              return;
            }
            setConfirming(true);
            // The confirm button mounts on the next render; focus follows it.
            requestAnimationFrame(() => confirm.current?.focus());
          }}
        >
          {action.pending ? "Flattening…" : "Flatten account"}
        </button>
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
          onClick={(event) => {
            if (event.detail > 1) {
              return;
            }
            action.run(() => api.flatten(address));
            setConfirming(false);
            // The confirm button goes with this render; focus stays on the status line, which says how it ended.
            requestAnimationFrame(() => status.current?.focus());
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

/**
 * The controls of the account at `address`; an account that is starting, flattening or halted takes none. The two
 * status lines are the same elements in every state, so what a control says, and where focus rests, stay put when
 * the account's state changes under it.
 */
export function AccountControls({ account, address }: { readonly account: AccountView; readonly address: string }) {
  const { api } = useServices();
  const running = account.state === "running";
  const toggle = useAction(address);
  const flatten = useAction(address);
  const flattenStatus = useRef<HTMLParagraphElement>(null);
  const offered = account.state === "running" || account.state === "stopped";
  return (
    <div>
      <div className="flex flex-wrap items-start gap-4">
        {offered ? (
          <>
            <ActionButton
              label={running ? "Stop account" : "Start account"}
              pendingLabel={running ? "Stopping…" : "Starting…"}
              action={toggle}
              call={() => api.setAccountRunning(address, !running)}
              showStatus={false}
            />
            <FlattenControl address={address} action={flatten} status={flattenStatus} />
          </>
        ) : (
          <p className="text-sm text-muted">{WAITING[account.state]}</p>
        )}
      </div>
      <ActionError error={toggle.error} warning={toggle.warning} done={toggle.done} />
      <ActionError error={flatten.error} warning={flatten.warning} done={flatten.done} statusRef={flattenStatus} />
    </div>
  );
}

/** Why an account in these states has no controls. */
const WAITING = {
  starting: "The account is reading the node; its controls appear once it follows the log.",
  flattening: "The account is closing its positions.",
  halted: "The account stopped on its own and takes no controls; see the reason above.",
} as const;
