/** Records a declaration for the selected accounts that lack one, only after the user reads and confirms it. */
import { useId, useState } from "react";

import type { DeclarationClass, FundingRead } from "@/api/funding-types";
import { confirmationKey, isBusy, selectedAccounts, undeclared } from "@/lib/funding";

import { ActionError, BUTTON_CLASS } from "./ActionButton";
import { useServices } from "./app-context";
import { OperationList } from "./OperationList";
import { Section } from "./Section";
import { useFundingAction } from "./use-funding-action";

interface DeclarationPanelProps {
  readonly read: FundingRead;
  readonly selected: ReadonlySet<string>;
}

/** The declaration to read and confirm, the accounts it applies to, and the button that records it. */
export function DeclarationPanel({ read, selected }: DeclarationPanelProps) {
  const { api } = useServices();
  const action = useFundingAction();
  const [chosen, setChosen] = useState<DeclarationClass | null>(null);
  const [confirmedKey, setConfirmedKey] = useState<string | null>(null);
  const groupId = useId();
  const targets = undeclared(selectedAccounts(read.accounts, selected));
  const option = read.declarations.find((candidate) => candidate.class === chosen) ?? null;
  // A confirmation names one class and these exact accounts; changing either makes it stale.
  const key = option === null ? null : confirmationKey(option.class, targets);
  const confirmed = key !== null && confirmedKey === key;
  const busy = targets.some((account) => isBusy(read.operations, "declare", account.address));
  const count = targets.length;
  const ready = option !== null && confirmed && count > 0 && !busy && !action.pending;
  return (
    <Section title="Declaration">
      <p className="mb-2 text-sm">
        An account declares once, before its first deposit credits. The declaration is signed with the account's own key.
        Nothing is recorded until you confirm it here.
      </p>
      <fieldset className="mb-2 flex flex-col gap-2">
        <legend className="text-sm font-medium">Statement to declare</legend>
        {read.declarations.map((declaration) => (
          <label key={declaration.class} className="flex items-start gap-2">
            <input
              type="radio"
              name={groupId}
              className="mt-1 size-5 accent-accent"
              checked={chosen === declaration.class}
              onChange={() => {
                setChosen(declaration.class);
              }}
            />
            <span>{declaration.statement}</span>
          </label>
        ))}
      </fieldset>
      {count === 0 ? (
        <p className="text-sm text-muted">Select accounts that have not declared.</p>
      ) : (
        <>
          <p className="text-sm">The declaration will be signed and recorded for:</p>
          <ul className="mb-2 text-sm">
            {targets.map((account) => (
              <li key={account.address} className="break-all"><span className="font-medium">{account.name}</span> {account.address}</li>
            ))}
          </ul>
        </>
      )}
      <label className="mb-2 flex items-start gap-2">
        <input
          type="checkbox"
          className="mt-1 size-5 accent-accent"
          checked={confirmed}
          disabled={key === null || count === 0}
          onChange={(event) => {
            setConfirmedKey(event.target.checked ? key : null);
          }}
        />
        <span>I have read the statement above and confirm it for the {String(count)} {count === 1 ? "account" : "accounts"} listed.</span>
      </label>
      <button
        type="button"
        className={BUTTON_CLASS}
        disabled={!ready}
        onClick={() => {
          if (option !== null) {
            action.run(
              () => api.declare(targets.map((account) => account.address), option),
              () => { setConfirmedKey(null); },
            );
          }
        }}
      >
        {action.pending ? "Recording…" : `Declare for ${String(count)} ${count === 1 ? "account" : "accounts"}`}
      </button>
      <ActionError error={action.error} />
      <OperationList kind="declare" operations={read.operations} accounts={read.accounts} />
    </Section>
  );
}
