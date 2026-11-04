/** Checks of a strategy's parameter edits against the bounds the bot serves. */
import { compareDecimals, isDecimal } from "./decimal";

/** A parameter's draft value and its bounds, as text. */
export interface ParamDraft {
  readonly name: string;
  readonly kind: "integer" | "decimal";
  readonly draft: string;
  readonly min: string;
  readonly max: string;
}

/** The checked values to send, or what was wrong, by parameter name. */
export type ParamCheck =
  | { readonly status: "ok"; readonly values: Readonly<Record<string, string | number>> }
  | { readonly status: "invalid"; readonly problems: Readonly<Record<string, string>> };

const INTEGER = /^\d{1,15}$/;

/** Why one draft is refused, or null when it is within its kind and bounds. */
export function problemWith(param: ParamDraft): string | null {
  const text = param.draft.trim();
  if (param.kind === "integer" ? !INTEGER.test(text) : !isDecimal(text)) {
    return param.kind === "integer" ? "Enter a whole number." : "Enter a decimal number.";
  }
  if (compareDecimals(text, param.min) < 0 || compareDecimals(text, param.max) > 0) {
    return `Enter a value from ${param.min} to ${param.max}.`;
  }
  return null;
}

/** Checks every draft; the bot checks again and its refusal wins. Integers go as numbers, decimals as strings. */
export function checkParams(drafts: readonly ParamDraft[]): ParamCheck {
  const problems: Record<string, string> = {};
  const values: Record<string, string | number> = {};
  for (const draft of drafts) {
    const problem = problemWith(draft);
    if (problem !== null) {
      problems[draft.name] = problem;
    } else {
      values[draft.name] = draft.kind === "integer" ? Number(draft.draft.trim()) : draft.draft.trim();
    }
  }
  return Object.keys(problems).length > 0 ? { status: "invalid", problems } : { status: "ok", values };
}
