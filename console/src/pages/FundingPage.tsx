/** Funding: each account's declaration, wallet, balance, MONKEY and withdrawals, and the actions on a selection. */
import { useState } from "react";

import { FundingActions } from "@/components/FundingActions";
import { FundingJobs } from "@/components/FundingJobs";
import { FundingTable } from "@/components/FundingTable";
import { Link } from "@/components/Link";
import { PageTitle } from "@/components/PageTitle";
import { useFunding } from "@/components/use-funding";

/** Shows what the accounts hold and owe, and acts on those selected. */
export function FundingPage() {
  const { funding, error, reload } = useFunding();
  const [ticked, setTicked] = useState<ReadonlySet<string>>(new Set());
  const accounts = funding?.enabled === true ? funding.accounts : [];
  // An account that left the list is not acted on.
  const selected = accounts.map((a) => a.address).filter((address) => ticked.has(address));
  return (
    <>
      <PageTitle>Funding</PageTitle>
      <p className="mb-4"><Link to="/">All accounts</Link></p>
      {error === null ? null : <p role="alert" className="mb-2 text-loss">{`The funding could not be read: ${error}`}</p>}
      {funding === null ? (
        error === null ? <p className="text-muted">Loading funding…</p> : null
      ) : !funding.enabled ? (
        <p className="text-muted">Funding is off: the bot's config has no [chain] section.</p>
      ) : funding.accounts.length === 0 ? (
        <p className="text-muted">No account has a key the bot could unlock.</p>
      ) : (
        <div className="flex flex-col gap-6">
          <p className="text-muted">
            An account the node had not credited, or that had not declared, when the bot started trades once the bot is
            restarted. Stopping the bot ends the actions that have not ended; what they already sent may still land.
          </p>
          <FundingTable
            accounts={funding.accounts}
            selected={ticked}
            onToggle={(address) => {
              setTicked((previous) => {
                const next = new Set(previous);
                if (!next.delete(address)) {
                  next.add(address);
                }
                return next;
              });
            }}
            onToggleAll={() => {
              setTicked(selected.length === accounts.length ? new Set() : new Set(accounts.map((a) => a.address)));
            }}
          />
          <FundingActions funding={funding} accounts={selected} reload={reload} />
          <section aria-labelledby="jobs-heading">
            <h2 id="jobs-heading" className="mb-2 text-lg font-semibold">Actions</h2>
            <FundingJobs jobs={funding.jobs} />
          </section>
        </div>
      )}
    </>
  );
}
