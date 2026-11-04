/** Deposit or withdraw one USDC amount for each selected account; the destination is always the account itself. */
import { useId, useState } from "react";

import type { FundingRead } from "@/api/funding-types";
import { isBusy, parseUsdcAmount, selectedAccounts } from "@/lib/funding";

import { ActionError, BUTTON_CLASS } from "./ActionButton";
import { useServices } from "./app-context";
import { OperationList } from "./OperationList";
import { Section } from "./Section";
import { useFundingAction } from "./use-funding-action";

interface AmountPanelProps {
  readonly kind: "deposit" | "withdraw";
  readonly read: FundingRead;
  readonly selected: ReadonlySet<string>;
}

/** What each kind says and does. */
const WORDS = {
  deposit: {
    title: "Deposit USDC",
    pending: "Depositing…",
    verb: "Deposit",
    explain: "Moves the amount from each selected account's own wallet into the same account.",
  },
  withdraw: {
    title: "Withdraw USDC",
    pending: "Withdrawing…",
    verb: "Withdraw",
    explain: "Queues the amount from each selected account and pays it to the account's own wallet.",
  },
} as const;

/** The amount field, its button, and the kind's transactions. */
export function AmountPanel({ kind, read, selected }: AmountPanelProps) {
  const { api } = useServices();
  const action = useFundingAction();
  const [typed, setTyped] = useState("");
  const inputId = useId();
  const noteId = useId();
  const words = WORDS[kind];
  const targets = selectedAccounts(read.accounts, selected);
  const amount = parseUsdcAmount(typed);
  const busy = targets.some((account) => isBusy(read.operations, kind, account.address));
  const count = targets.length;
  const ready = amount.status === "ok" && count > 0 && !busy && !action.pending;
  return (
    <Section title={words.title}>
      <p className="mb-2 text-sm">{words.explain}</p>
      <form
        className="flex flex-wrap items-end gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (amount.status === "ok" && ready) {
            const value = amount.value;
            const addresses = targets.map((account) => account.address);
            action.run(() => (kind === "deposit" ? api.deposit(addresses, value) : api.withdraw(addresses, value)));
          }
        }}
      >
        <div className="flex flex-col">
          <label htmlFor={inputId} className="text-sm">Amount per account (USDC)</label>
          <input
            id={inputId}
            type="text"
            inputMode="decimal"
            autoComplete="off"
            maxLength={40}
            value={typed}
            aria-describedby={noteId}
            aria-invalid={typed !== "" && amount.status === "invalid"}
            className="min-h-9 w-44 rounded border border-line bg-surface px-2 text-right focus-visible:outline-2 focus-visible:outline-accent"
            onChange={(event) => {
              setTyped(event.target.value);
            }}
          />
        </div>
        <button type="submit" className={BUTTON_CLASS} disabled={!ready}>
          {action.pending ? words.pending : `${words.verb} for ${String(count)} ${count === 1 ? "account" : "accounts"}`}
        </button>
      </form>
      <p id={noteId} aria-live="polite" className="min-h-5 text-sm text-loss">
        {typed !== "" && amount.status === "invalid" ? amount.message : ""}
      </p>
      <ActionError error={action.error} />
      {count === 0 ? <p className="text-sm text-muted">Select the accounts to {kind} for.</p> : null}
      <OperationList kind={kind} operations={read.operations} accounts={read.accounts} />
    </Section>
  );
}
