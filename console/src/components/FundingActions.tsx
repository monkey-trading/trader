/** The four actions, each for the accounts selected. */
import { useId, useState } from "react";

import type { Api } from "@/api/client";
import { CLASSES, type DepositFrom, type EligibilityClass, type Funding } from "@/api/funding-types";

import { BUTTON_CLASS } from "./ActionButton";
import { useServices } from "./app-context";
import { useFundingAction, type FundingAction } from "./use-funding-action";

const FIELD = "min-h-9 rounded border border-line bg-surface px-2 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent";
const CLASS_NAMES: Readonly<Record<EligibilityClass, string>> = { professional: "Professional client", eligible_client: "Eligible client" };

/** What an amount does to an account that trades under a daily loss limit, which counts the day's change in equity. */
const DEPOSIT_NOTE = "A deposit counts toward the account's loss for the day: it hides a loss that has already happened.";
const WITHDRAW_NOTE =
  "A withdrawal counts toward the account's loss for the day. One larger than what is left of the account's daily loss limit closes its positions and stops it until the next UTC day.";

interface Props {
  readonly funding: Extract<Funding, { enabled: true }>;
  /** The addresses selected. */
  readonly accounts: readonly string[];
  readonly reload: () => void;
}

/** What a request came to: why it was refused, or how many jobs it started, in one live region that stays mounted. */
function Result({ action }: { readonly action: FundingAction }) {
  const text =
    action.error !== null
      ? `Failed: ${action.error}`
      : action.started === null
        ? ""
        : `Started ${String(action.started)} ${action.started === 1 ? "job" : "jobs"}; their states are listed below.`;
  return (
    <p aria-live="polite" className={`min-h-5 text-sm ${action.error === null ? "text-muted" : "text-loss"}`}>
      {text}
    </p>
  );
}

function Submit({ label, pendingLabel, action, disabled, call }: {
  readonly label: string;
  readonly pendingLabel: string;
  readonly action: FundingAction;
  readonly disabled: boolean;
  readonly call: (api: Api) => ReturnType<Api["claim"]>;
}) {
  const { api } = useServices();
  return (
    <button
      type="button"
      className={BUTTON_CLASS}
      disabled={disabled || action.pending}
      onClick={(event) => {
        if (event.detail <= 1) {
          action.run(() => call(api));
        }
      }}
    >
      {action.pending ? pendingLabel : label}
    </button>
  );
}

function plural(count: number): string {
  return `${String(count)} selected ${count === 1 ? "account" : "accounts"}`;
}

/** Declares the class the user chooses after reading its statement and confirming it. */
function DeclareForm({ funding, accounts, reload }: Props) {
  const action = useFundingAction(reload);
  const [cls, setCls] = useState<EligibilityClass | null>(null);
  // What the user confirmed: the class and the accounts it was confirmed for, so a change of either asks again.
  const [confirmedFor, setConfirmedFor] = useState<string | null>(null);
  const covers = `${cls ?? ""}|${accounts.join(",")}`;
  const confirmed = cls !== null && confirmedFor === covers;
  const group = useId();
  return (
    <fieldset className="flex flex-col gap-2 rounded border border-line p-3">
      <legend className="px-1 font-semibold">Declare</legend>
      {CLASSES.map((option) => (
        <label key={option} className="flex flex-wrap items-start gap-x-2">
          <input
            type="radio"
            name={group}
            checked={cls === option}
            onChange={() => { setCls(option); }}
            className="mt-1 size-5"
          />
          <span className="font-medium">{CLASS_NAMES[option]}</span>
          <span className="text-muted">{funding.statements[option]}</span>
        </label>
      ))}
      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          checked={confirmed}
          disabled={cls === null}
          onChange={(event) => { setConfirmedFor(event.target.checked ? covers : null); }}
          className="size-5"
        />
        {`I have read the statement and confirm it for ${plural(accounts.length)}`}
      </label>
      <div>
        <Submit
          label="Declare"
          pendingLabel="Declaring…"
          action={action}
          disabled={cls === null || !confirmed || accounts.length === 0}
          call={(api) => {
            if (cls === null) {
              throw new Error("declare was pressed with no class chosen");
            }
            // One confirmation covers one request.
            setConfirmedFor(null);
            return api.declare(accounts, cls, funding.statements[cls]);
          }}
        />
        <Result action={action} />
      </div>
    </fieldset>
  );
}

/** Deposits an amount of USDC for each account. */
function DepositForm({ funding, accounts, reload }: Props) {
  const action = useFundingAction(reload);
  const [amount, setAmount] = useState("");
  const [from, setFrom] = useState<DepositFrom>("account");
  const amountId = useId();
  const fromId = useId();
  const noteId = useId();
  return (
    <fieldset className="flex flex-col gap-2 rounded border border-line p-3">
      <legend className="px-1 font-semibold">Deposit</legend>
      <label htmlFor={amountId}>Amount of USDC for each account</label>
      <input id={amountId} aria-describedby={noteId} inputMode="decimal" value={amount} onChange={(e) => { setAmount(e.target.value.trim()); }} className={FIELD} />
      <p id={noteId} className="text-muted">{DEPOSIT_NOTE}</p>
      <label htmlFor={fromId}>From</label>
      <select id={fromId} value={from} onChange={(e) => { setFrom(e.target.value === "funding" ? "funding" : "account"); }} className={FIELD}>
        <option value="account">The account's own wallet</option>
        {funding.fundingKey ? <option value="funding">The funding wallet</option> : null}
      </select>
      <div>
        <Submit
          label="Deposit"
          pendingLabel="Depositing…"
          action={action}
          disabled={amount === "" || accounts.length === 0}
          call={(api) => api.deposit(accounts, amount, from)}
        />
        <Result action={action} />
      </div>
    </fieldset>
  );
}

/** Claims each account's MONKEY, and withdraws USDC from each. */
function PayoutForms({ accounts, reload }: Props) {
  const claim = useFundingAction(reload);
  const withdraw = useFundingAction(reload);
  const [amount, setAmount] = useState("");
  const amountId = useId();
  const noteId = useId();
  return (
    <>
      <fieldset className="flex flex-col gap-2 rounded border border-line p-3">
        <legend className="px-1 font-semibold">Claim MONKEY</legend>
        <p className="text-muted">Each account receives its own MONKEY.</p>
        <div>
          <Submit label="Claim" pendingLabel="Claiming…" action={claim} disabled={accounts.length === 0} call={(api) => api.claim(accounts)} />
          <Result action={claim} />
        </div>
      </fieldset>
      <fieldset className="flex flex-col gap-2 rounded border border-line p-3">
        <legend className="px-1 font-semibold">Withdraw</legend>
        <label htmlFor={amountId}>Amount of USDC from each account; empty pays only what the vault has approved</label>
        <input id={amountId} aria-describedby={noteId} inputMode="decimal" value={amount} onChange={(e) => { setAmount(e.target.value.trim()); }} className={FIELD} />
        <p className="text-muted">Each account is paid at its own address.</p>
        <p id={noteId} className="text-muted">{WITHDRAW_NOTE}</p>
        <div>
          <Submit
            label="Withdraw"
            pendingLabel="Withdrawing…"
            action={withdraw}
            disabled={accounts.length === 0}
            call={(api) => api.withdraw(accounts, amount === "" ? null : amount)}
          />
          <Result action={withdraw} />
        </div>
      </fieldset>
    </>
  );
}

/** The forms, which act on the selected accounts. */
export function FundingActions(props: Props) {
  return (
    <div className="grid gap-4 md:grid-cols-2">
      <DeclareForm {...props} />
      <DepositForm {...props} />
      <PayoutForms {...props} />
    </div>
  );
}
