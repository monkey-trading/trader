/** The parameter form of one strategy: fields checked against the bot's bounds, saved on submit. */
import { useId, useState, type SubmitEvent } from "react";

import type { Param } from "@/api/types";
import { checkParams } from "@/lib/params";

import { BUTTON_CLASS } from "./ActionButton";
import { useServices } from "./app-context";
import { useAction } from "./use-action";

interface ParamsFormProps {
  readonly address: string;
  readonly strategyId: string;
  readonly params: readonly Param[];
}

/** Edits the parameters. The parent keys this by the saved values, so a save resets the drafts. */
export function ParamsForm({ address, strategyId, params }: ParamsFormProps) {
  const { api } = useServices();
  const [drafts, setDrafts] = useState<Readonly<Record<string, string>>>(
    () => Object.fromEntries(params.map((param) => [param.name, param.value])),
  );
  const [problems, setProblems] = useState<Readonly<Record<string, string>>>({});
  const action = useAction();
  const prefix = useId();

  const onSubmit = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    const checked = checkParams(params.map((param) => ({ ...param, draft: drafts[param.name] ?? "" })));
    if (checked.status === "invalid") {
      setProblems(checked.problems);
      return;
    }
    setProblems({});
    action.run(() => api.setParams(address, strategyId, checked.values));
  };

  return (
    <form onSubmit={onSubmit} className="mt-2 flex flex-col gap-2">
      {params.map((param) => {
        const id = `${prefix}-${param.name}`;
        const problem = problems[param.name];
        return (
          <div key={param.name} className="flex flex-col gap-1">
            <label htmlFor={id} className="text-sm font-medium">
              {param.name}
            </label>
            <input
              id={id}
              type="text"
              inputMode="decimal"
              maxLength={32}
              value={drafts[param.name] ?? ""}
              aria-invalid={problem !== undefined}
              aria-describedby={`${id}-help`}
              onChange={(event) => {
                setDrafts({ ...drafts, [param.name]: event.target.value });
              }}
              className="min-h-9 rounded border border-line bg-surface px-2 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
            />
            <p id={`${id}-help`} className={`text-sm ${problem === undefined ? "text-muted" : "text-loss"}`}>
              {problem ?? `From ${param.min} to ${param.max}.`}
            </p>
          </div>
        );
      })}
      <div>
        <button type="submit" className={BUTTON_CLASS} disabled={action.pending}>
          {action.pending ? "Saving…" : "Save parameters"}
        </button>
        <p aria-live="polite" className="min-h-5 text-sm text-loss">
          {action.error === null ? "" : `Failed: ${action.error}`}
        </p>
      </div>
    </form>
  );
}
