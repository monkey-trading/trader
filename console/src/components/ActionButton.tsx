/** A button for an `Action`, with the failure it caused shown under it. */
import type { Ref } from "react";

import type { Outcome, Warned } from "@/api/client";

import type { Action } from "./use-action";

interface ActionButtonProps {
  readonly label: string;
  /** What the button reads while the call is in flight. */
  readonly pendingLabel: string;
  readonly action: Action;
  /** The call the button makes. */
  readonly call: () => Promise<Outcome<Warned | null>>;
  /** Whether the button shows its own status line; a parent that keeps the line elsewhere says no. */
  readonly showStatus?: boolean;
}

/** Shared look of every button. */
export const BUTTON_CLASS =
  "min-h-9 min-w-9 rounded border border-line bg-surface px-3 py-1 font-medium text-ink hover:bg-paper focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:cursor-wait disabled:opacity-60";

/**
 * The failure of an action, or the warning of one that worked, in a polite live region that is always present.
 * A warning does not read as a failure.
 */
export function ActionError({
  error,
  warning = null,
  done = false,
  statusRef,
}: {
  readonly error: string | null;
  readonly warning?: string | null;
  /** The attempt worked: say so, since a control that works changes nothing a screen reader hears. */
  readonly done?: boolean;
  /** Lets a control move focus here, a node that stays when the control that held it goes. */
  readonly statusRef?: Ref<HTMLParagraphElement>;
}) {
  const text = error !== null ? `Failed: ${error}` : warning !== null ? `Done, with a warning: ${warning}` : done ? "Done." : "";
  return (
    <p ref={statusRef} tabIndex={-1} aria-live="polite" className={`min-h-5 text-sm outline-none ${error === null && warning !== null ? "text-warn" : error === null ? "text-muted" : "text-loss"}`}>
      {text}
    </p>
  );
}

/** A button that runs `action`, cannot be pressed again while pending, and shows its failure in place. */
export function ActionButton({ label, pendingLabel, action, call, showStatus = true }: ActionButtonProps) {
  return (
    <div>
      <button
        type="button"
        className={BUTTON_CLASS}
        disabled={action.pending}
        onClick={(event) => {
          // The second click of a double-click lands on the button as the first left it, or on the opposite one.
          if (event.detail <= 1) {
            action.run(call);
          }
        }}
      >
        {action.pending ? pendingLabel : label}
      </button>
      {showStatus ? <ActionError error={action.error} warning={action.warning} done={action.done} /> : null}
    </div>
  );
}
