/** Claims MONKEY for the selected accounts that have some; the vault mints it to each account itself. */
import type { FundingRead } from "@/api/funding-types";
import { isBusy, selectedAccounts, withUnclaimed } from "@/lib/funding";

import { ActionError, BUTTON_CLASS } from "./ActionButton";
import { useServices } from "./app-context";
import { OperationList } from "./OperationList";
import { Section } from "./Section";
import { useFundingAction } from "./use-funding-action";

interface PanelProps {
  readonly read: FundingRead;
  readonly selected: ReadonlySet<string>;
}

/** The claim control and its transactions. */
export function ClaimPanel({ read, selected }: PanelProps) {
  const { api } = useServices();
  const action = useFundingAction();
  const targets = withUnclaimed(selectedAccounts(read.accounts, selected));
  const busy = targets.some((account) => isBusy(read.operations, "claim", account.address));
  const count = targets.length;
  return (
    <Section title="Claim MONKEY">
      <p className="mb-2 text-sm">
        Mints each selected account's unclaimed MONKEY to the account's own address.
        {read.gasPayer === null ? " Each account pays its own gas." : ` ${read.gasPayer} pays the gas and receives nothing.`}
      </p>
      <button
        type="button"
        className={BUTTON_CLASS}
        disabled={count === 0 || busy || action.pending}
        onClick={() => {
          action.run(() => api.claim(targets.map((account) => account.address)));
        }}
      >
        {action.pending ? "Claiming…" : `Claim for ${String(count)} ${count === 1 ? "account" : "accounts"}`}
      </button>
      <ActionError error={action.error} />
      {count === 0 ? <p className="text-sm text-muted">Select accounts with unclaimed MONKEY.</p> : null}
      <OperationList kind="claim" operations={read.operations} accounts={read.accounts} />
    </Section>
  );
}
