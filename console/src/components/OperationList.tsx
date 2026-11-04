/** The recent operations of one kind, each with its account, state and transaction until it lands. */
import type { FundingAccount, Operation, OperationKind } from "@/api/funding-types";
import { operationsOfKind, operationStatus } from "@/lib/funding";
import { shortAddress } from "@/lib/format";

interface OperationListProps {
  readonly kind: OperationKind;
  readonly operations: readonly Operation[];
  readonly accounts: readonly FundingAccount[];
}

/** One operation's line: the account, what it is doing or why it failed, and its transaction. */
function OperationLine({ operation, name }: { readonly operation: Operation; readonly name: string }) {
  return (
    <li className={operation.state === "failed" ? "text-loss" : ""}>
      <span className="font-medium">{name}</span>: {operationStatus(operation)}
      {operation.txHash === null ? "" : <span className="ml-2 text-muted">tx {shortAddress(operation.txHash)}</span>}
    </li>
  );
}

/** A polite live region that exists before it changes, so a landing or a revert is announced. */
export function OperationList({ kind, operations, accounts }: OperationListProps) {
  const shown = operationsOfKind(operations, kind);
  const nameOf = (address: string) => accounts.find((account) => account.address === address)?.name ?? shortAddress(address);
  return (
    <ul aria-live="polite" aria-label={`Recent ${kind} transactions`} className="mt-2 flex flex-col gap-1 text-sm">
      {shown.map((operation) => (
        <OperationLine key={operation.id} operation={operation} name={nameOf(operation.address)} />
      ))}
    </ul>
  );
}
