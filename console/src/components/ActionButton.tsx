/** A button for an `Action`, with the failure it caused shown under it. */
import type { Outcome } from "@/api/client";

import type { Action } from "./use-action";

interface ActionButtonProps {
  readonly label: string;
  /** What the button reads while the call is in flight. */
  readonly pendingLabel: string;
  readonly action: Action;
  /** The call the button makes. */
  readonly call: () => Promise<Outcome<null>>;
}

/** Shared look of every button. */
export const BUTTON_CLASS =
  "min-h-9 min-w-9 rounded border border-line bg-surface px-3 py-1 font-medium text-ink hover:bg-paper focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:cursor-wait disabled:opacity-60";

/** The failure text of an action, in a polite live region that is always present. */
export function ActionError({ error }: { readonly error: string | null }) {
  return (
    <p aria-live="polite" className="min-h-5 text-sm text-loss">
      {error === null ? "" : `Failed: ${error}`}
    </p>
  );
}

/** A button that runs `action`, cannot be pressed again while pending, and shows its failure in place. */
export function ActionButton({ label, pendingLabel, action, call }: ActionButtonProps) {
  return (
    <div>
      <button
        type="button"
        className={BUTTON_CLASS}
        disabled={action.pending}
        onClick={() => {
          action.run(call);
        }}
      >
        {action.pending ? pendingLabel : label}
      </button>
      <ActionError error={action.error} />
    </div>
  );
}
