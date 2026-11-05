/** Checks of a strategy's period edits against the bounds the bot serves. */

/** A parameter's draft value and its inclusive bounds. */
export interface ParamDraft {
  readonly name: string;
  readonly draft: string;
  readonly bounds: readonly [number, number];
}

/** The checked values to send, or what was wrong, by parameter name. */
export type ParamCheck =
  | { readonly status: "ok"; readonly values: Readonly<Record<string, number>> }
  | { readonly status: "invalid"; readonly problems: Readonly<Record<string, string>> };

const WHOLE_NUMBER = /^\d{1,9}$/;

/** Why one draft is refused, or null when it is a whole number within its bounds. */
export function problemWith(param: ParamDraft): string | null {
  const text = param.draft.trim();
  if (!WHOLE_NUMBER.test(text)) {
    return "Enter a whole number.";
  }
  const [low, high] = param.bounds;
  const value = Number(text);
  return value < low || value > high ? `Enter a value from ${String(low)} to ${String(high)}.` : null;
}

/** Checks every draft; the bot checks again and its refusal wins. */
export function checkParams(drafts: readonly ParamDraft[]): ParamCheck {
  const problems: Record<string, string> = {};
  const values: Record<string, number> = {};
  for (const draft of drafts) {
    const problem = problemWith(draft);
    if (problem !== null) {
      problems[draft.name] = problem;
    } else {
      values[draft.name] = Number(draft.draft.trim());
    }
  }
  return Object.keys(problems).length > 0 ? { status: "invalid", problems } : { status: "ok", values };
}
