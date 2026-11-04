/** Funding: each account's declaration, wallet, balance and MONKEY, and the actions on a selection of accounts. */
import { useEffect, useState } from "react";

import { useFunding, useServices } from "@/components/app-context";
import { AmountPanel } from "@/components/AmountPanel";
import { ClaimPanel } from "@/components/ClaimPanel";
import { DeclarationPanel } from "@/components/DeclarationPanel";
import { FundingTable } from "@/components/FundingTable";
import { Link } from "@/components/Link";
import { PageTitle } from "@/components/PageTitle";

/** Adds `address` to the selection, or removes it. */
function toggled(selected: ReadonlySet<string>, address: string): ReadonlySet<string> {
  const next = new Set(selected);
  if (!next.delete(address)) {
    next.add(address);
  }
  return next;
}

/** The funding page. */
export function FundingPage() {
  const { funding } = useServices();
  const { status, read, failure } = useFunding();
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  useEffect(() => funding.start(), [funding]);

  const accounts = read?.accounts ?? [];
  const allSelected = accounts.length > 0 && accounts.every((account) => selected.has(account.address));
  return (
    <>
      <p className="mb-2"><Link to="/">All accounts</Link></p>
      <PageTitle>Funding</PageTitle>
      <p className="mb-4 max-w-prose text-sm">
        Every action here pays only the account's own address. Deposits come from the account's own wallet, claims
        mint MONKEY to the account, and withdrawals pay the account's own wallet. No control can name another address.
      </p>
      <p role="status" className="mb-2 text-sm text-muted">
        {status === "loading" ? "Loading funds…" : ""}
        {status === "offline" ? `The bot did not answer, so these figures may be out of date: ${failure ?? ""}` : ""}
      </p>
      {read === null ? null : accounts.length === 0 ? (
        <p className="text-muted">The bot has no accounts.</p>
      ) : (
        <div className={`flex flex-col gap-4 ${status === "live" ? "" : "opacity-70"}`}>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              className="size-5 accent-accent"
              checked={allSelected}
              onChange={() => {
                setSelected(allSelected ? new Set() : new Set(accounts.map((account) => account.address)));
              }}
            />
            Select all accounts
          </label>
          <FundingTable accounts={accounts} selected={selected} onToggle={(address) => { setSelected(toggled(selected, address)); }} />
          <DeclarationPanel read={read} selected={selected} />
          <AmountPanel kind="deposit" read={read} selected={selected} />
          <ClaimPanel read={read} selected={selected} />
          <AmountPanel kind="withdraw" read={read} selected={selected} />
        </div>
      )}
    </>
  );
}
