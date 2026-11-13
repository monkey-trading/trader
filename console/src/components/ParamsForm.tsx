/** The period form of one strategy: fields checked against the bot's bounds, saved on submit. */
import { useId, useState, type SubmitEvent } from "react";

import type { Param } from "@/api/types";
import { checkParams, wireValue } from "@/lib/params";

import { BUTTON_CLASS } from "./ActionButton";
import { useServices } from "./app-context";
import { useAction } from "./use-action";

interface ParamsFormProps {
  readonly address: string;
  readonly strategyId: number;
  /** The strategy's kind, which the bot's parameter body names as the config does. */
  readonly kind: string;
  readonly params: readonly Param[];
}

/** Edits the periods. The parent keys this by the saved values, so a save resets the drafts. */
export function ParamsForm({ address, strategyId, kind, params }: ParamsFormProps) {
  const { api } = useServices();
  const [drafts, setDrafts] = useState<Readonly<Record<string, string>>>(
    () => Object.fromEntries(params.map((param) => [param.name, param.value])),
  );
  const [problems, setProblems] = useState<Readonly<Record<string, string>>>({});
  const action = useAction(address);
  const prefix = useId();

  const editable = params.flatMap((param) => (param.bounds === null ? [] : [{ ...param, bounds: param.bounds }]));
  if (editable.length === 0) {
    return (
      <p className="mt-2 text-sm text-muted">
        {params.map((param) => `${param.name} ${param.value}`).join(", ")}. The bot's config does not let the
        console change these.
      </p>
    );
  }

  const onSubmit = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    const checked = checkParams(editable.map((param) => ({ name: param.name, kind: param.kind, draft: drafts[param.name] ?? "", bounds: param.bounds })));
    if (checked.status === "invalid") {
      setProblems(checked.problems);
      // Focus goes to the first field that is wrong, whose help line, linked to it, says why.
      const first = editable.find((param) => checked.problems[param.name] !== undefined);
      const form = event.currentTarget;
      if (first !== undefined) {
        requestAnimationFrame(() => {
          const field = form.elements.namedItem(`${prefix}-${first.name}`);
          if (field instanceof HTMLElement) {
            field.focus();
          }
        });
      }
      return;
    }
    setProblems({});
    // The bot takes every parameter together, named by the strategy's kind; one that is not editable keeps its value.
    const all = Object.fromEntries(params.map((param) => [param.name, wireValue(param.kind, param.value)]));
    action.run(() => api.setParams(address, strategyId, { kind, ...all, ...checked.values }));
  };

  return (
    <form onSubmit={onSubmit} className="mt-2 flex flex-col gap-2">
      {editable.map((param) => {
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
              {problem ?? `From ${param.bounds[0]} to ${param.bounds[1]}.`}
            </p>
          </div>
        );
      })}
      <div>
        <button type="submit" className={BUTTON_CLASS} disabled={action.pending}>
          {action.pending ? "Saving…" : "Save parameters"}
        </button>
        <p aria-live="polite" className={`min-h-5 text-sm ${action.error === null ? "text-muted" : "text-loss"}`}>
          {action.error !== null ? `Failed: ${action.error}` : action.done ? "Saved." : ""}
        </p>
      </div>
    </form>
  );
}
