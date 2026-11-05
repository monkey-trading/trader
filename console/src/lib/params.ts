/** Checks of a strategy's parameter edits against the bounds the bot serves. */
import { compareDecimals, isDecimal } from "./decimal";

/** A parameter's draft value, its kind and its inclusive bounds. */
export interface ParamDraft {
  readonly name: string;
  readonly kind: "integer" | "decimal";
  readonly draft: string;
  readonly bounds: readonly [string, string];
}

/** The checked values to send, or what was wrong, by parameter name. */
export type ParamCheck =
  | { readonly status: "ok"; readonly values: Readonly<Record<string, string | number>> }
  | { readonly status: "invalid"; readonly problems: Readonly<Record<string, string>> };

const WHOLE_NUMBER = /^\d{1,9}$/;

/** Why one draft is refused, or null when it is of its kind and within its bounds. */
export function problemWith(param: ParamDraft): string | null {
  const text = param.draft.trim();
  if (param.kind === "integer" ? !WHOLE_NUMBER.test(text) : !isDecimal(text)) {
    return param.kind === "integer" ? "Enter a whole number." : "Enter a decimal number.";
  }
  const [low, high] = param.bounds;
  return compareDecimals(text, low) < 0 || compareDecimals(text, high) > 0 ? `Enter a value from ${low} to ${high}.` : null;
}

/** A value as the config writes it: a whole number as a number, a decimal as a string. */
export function wireValue(kind: "integer" | "decimal", value: string): string | number {
  return kind === "integer" ? Number(value) : value;
}

/** Checks every draft; the bot checks again and its refusal wins. */
export function checkParams(drafts: readonly ParamDraft[]): ParamCheck {
  const problems: Record<string, string> = {};
  const values: Record<string, string | number> = {};
  for (const draft of drafts) {
    const problem = problemWith(draft);
    if (problem !== null) {
      problems[draft.name] = problem;
    } else {
      values[draft.name] = wireValue(draft.kind, draft.draft.trim());
    }
  }
  return Object.keys(problems).length > 0 ? { status: "invalid", problems } : { status: "ok", values };
}
