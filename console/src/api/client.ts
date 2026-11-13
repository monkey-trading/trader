/** The console's only door to the bot: every route, request and response check lives here. */
import { readFunding, readJobs } from "./funding-parsers";
import type { DepositFrom, EligibilityClass, Funding, FundingJob } from "./funding-types";
import { parseWith, readAccount, readError, readFills, type Parsed } from "./parsers";
import type { AccountView, Fill } from "./types";

/** What a call came to. A refusal or an outage is a value the control shows, not an exception. */
export type Outcome<T> =
  | { readonly status: "ok"; readonly value: T }
  | {
      readonly status: "unauthenticated";
      /** The bot's code for the 401, when its body has one: `wrong_password` on a login. */
      readonly code?: string | null;
    }
  | {
      readonly status: "refused";
      readonly message: string;
      /** The bot's code for the refusal, when its body has one. */
      readonly code?: string | null;
      readonly retryAfterSeconds: number | null;
    }
  | { readonly status: "unavailable"; readonly message: string; readonly code?: string | null };

/** A control that did its work but says something the user should read: the flatten whose stop was not saved. */
export interface Warned {
  readonly warning: string;
}

/**
 * The code of the bot's 500 answer to a flatten that ran but whose stop was not saved. The flatten ran, so it is
 * not a failure; the answer's message is shown as a warning.
 */
const FLATTENED_STOP_UNSAVED = "flattened_stop_unsaved";
/** The bot's code for a stop that holds but could not be saved. */
const STOP_UNSAVED = "stop_unsaved";

/** The calls the stores and pages make. A control answers `null`, or a warning about what it did. */
export interface Api {
  session(): Promise<Outcome<null>>;
  login(password: string): Promise<Outcome<null>>;
  logout(): Promise<Outcome<null>>;
  account(address: string): Promise<Outcome<AccountView>>;
  fills(address: string): Promise<Outcome<readonly Fill[]>>;
  setAccountRunning(address: string, running: boolean): Promise<Outcome<Warned | null>>;
  flatten(address: string): Promise<Outcome<Warned | null>>;
  setStrategyRunning(address: string, strategyId: number, running: boolean): Promise<Outcome<Warned | null>>;
  setParams(address: string, strategyId: number, params: Readonly<Record<string, string | number>>): Promise<Outcome<Warned | null>>;
  /** What each account holds and owes, and the funding jobs. */
  funding(): Promise<Outcome<Funding>>;
  /** Each action starts one job per account, answered with the jobs as they stand. */
  declare(accounts: readonly string[], cls: EligibilityClass, statement: string): Promise<Outcome<readonly FundingJob[]>>;
  deposit(accounts: readonly string[], amount: string, from: DepositFrom): Promise<Outcome<readonly FundingJob[]>>;
  claim(accounts: readonly string[]): Promise<Outcome<readonly FundingJob[]>>;
  withdraw(accounts: readonly string[], amount: string | null): Promise<Outcome<readonly FundingJob[]>>;
}

/** The routes of the bot's console API, the one place their paths are written. */
export const routes = {
  session: "/api/session",
  login: "/api/login",
  logout: "/api/logout",
  account: (address: string) => `/api/accounts/${encodeURIComponent(address)}`,
  fills: (address: string) => `/api/accounts/${encodeURIComponent(address)}/fills`,
  accountRun: (address: string, action: "start" | "stop") => `/api/accounts/${encodeURIComponent(address)}/${action}`,
  flatten: (address: string) => `/api/accounts/${encodeURIComponent(address)}/flatten`,
  strategyRun: (address: string, id: number, action: "start" | "stop") =>
    `/api/accounts/${encodeURIComponent(address)}/strategies/${String(id)}/${action}`,
  strategyParams: (address: string, id: number) =>
    `/api/accounts/${encodeURIComponent(address)}/strategies/${String(id)}/params`,
  funding: "/api/funding",
  fundingAction: (action: "declare" | "deposit" | "claim" | "withdraw") => `/api/funding/${action}`,
  /** The live feed's path, on the page's own origin. */
  feed: "/api/live",
} as const;

/** The page's `fetch`, narrowed to the calls the client makes. */
type Fetch = (path: string, init?: RequestInit) => Promise<Response>;

/** Reads the whole-seconds `Retry-After` header, or null. */
function retryAfter(response: Response): number | null {
  const seconds = Number(response.headers.get("Retry-After"));
  return Number.isSafeInteger(seconds) && seconds > 0 ? seconds : null;
}

/** The words for an error answer that carries no readable message: a body too slow or too large has none. */
function fallbackMessage(status: number): string {
  switch (status) {
    case 408:
      return "The request took too long to send, so the bot dropped it.";
    case 413:
      return "The request was too large for the bot to take.";
    default:
      return `The bot refused the request (status ${String(status)}).`;
  }
}

/** Why there is no answer: the deadline passed, or the bot could not be reached. A control may still have acted. */
function unanswered(signal: AbortSignal, method: "GET" | "POST" | "PUT"): string {
  if (!signal.aborted) {
    return "The bot did not answer.";
  }
  return method === "GET"
    ? "The bot did not answer in time."
    : "The bot did not answer in time; it may still act on the request.";
}

/** Turns a parse result into an outcome; a body that fails its check is an outage, never data. */
function checked<T>(parsed: Parsed<T>): Outcome<T> {
  return parsed.status === "ok"
    ? { status: "ok", value: parsed.value }
    : { status: "unavailable", message: `The bot sent an answer this console cannot read (${parsed.message}).` };
}

/** How long a request may take, answer included, before the console gives up on it. */
const DEADLINE_MS = 15_000;
/** A flatten waits for the log and makes several calls to the node: the bot itself allows it two minutes. */
const FLATTEN_DEADLINE_MS = 130_000;

/** One request; `read` checks the success body; it is abandoned after `deadlineMs`, answer body included. */
async function call<T>(
  fetchFn: Fetch,
  method: "GET" | "POST" | "PUT",
  path: string,
  body: unknown,
  read: (body: unknown) => T,
  deadlineMs = DEADLINE_MS,
): Promise<Outcome<T>> {
  const signal = AbortSignal.timeout(deadlineMs);
  let response: Response;
  try {
    response = await fetchFn(path, {
      method,
      credentials: "same-origin",
      headers: body === undefined ? undefined : { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal,
    });
  } catch {
    return { status: "unavailable", message: unanswered(signal, method) };
  }
  let json: unknown;
  try {
    json = await response.json();
  } catch {
    // An answer that stalled or is no JSON: a stalled one is the deadline's, and is no answer.
    if (signal.aborted) {
      return { status: "unavailable", message: unanswered(signal, method) };
    }
    json = null;
  }
  if (response.status === 401) {
    return { status: "unauthenticated", code: readError(json)?.code ?? null };
  }
  if (response.ok) {
    return checked(parseWith(read, json));
  }
  const error = readError(json);
  const message = error?.message ?? fallbackMessage(response.status);
  const code = error?.code ?? null;
  return response.status >= 500
    ? { status: "unavailable", message, code }
    : { status: "refused", message, code, retryAfterSeconds: retryAfter(response) };
}

/** Reads no body: the session, login, logout and control answers are 204s with nothing the console uses. */
const nothing = (): null => null;

/** A flatten, or a stop, that took effect but answered 500 because it could not be saved is a success that carries the warning. */
function savedOutcome(outcome: Outcome<Warned | null>, code: string): Outcome<Warned | null> {
  return outcome.status === "unavailable" && outcome.code === code
    ? { status: "ok", value: { warning: outcome.message } }
    : outcome;
}

/**
 * Builds the API client over `fetchFn` (the page's `fetch`; tests pass a stand-in). `deadlineMs` replaces every
 * request's deadline, for tests.
 */
export function createApi(fetchFn: Fetch, deadlineMs?: number): Api {
  const control = (
    method: "POST" | "PUT",
    path: string,
    body: unknown,
    limit?: number,
  ): Promise<Outcome<Warned | null>> => call(fetchFn, method, path, body, nothing, deadlineMs ?? limit);
  return {
    session: () => call(fetchFn, "GET", routes.session, undefined, nothing, deadlineMs),
    login: (password) => call(fetchFn, "POST", routes.login, { password }, nothing, deadlineMs),
    logout: () => call(fetchFn, "POST", routes.logout, {}, nothing, deadlineMs),
    account: (address) => call(fetchFn, "GET", routes.account(address), undefined, readAccount, deadlineMs),
    fills: (address) => call(fetchFn, "GET", routes.fills(address), undefined, readFills, deadlineMs),
    setAccountRunning: async (address, running) =>
      running
        ? control("POST", routes.accountRun(address, "start"), {})
        : savedOutcome(await control("POST", routes.accountRun(address, "stop"), {}), STOP_UNSAVED),
    flatten: async (address) => savedOutcome(await control("POST", routes.flatten(address), {}, FLATTEN_DEADLINE_MS), FLATTENED_STOP_UNSAVED),
    setStrategyRunning: async (address, id, running) =>
      running
        ? control("POST", routes.strategyRun(address, id, "start"), {})
        : savedOutcome(await control("POST", routes.strategyRun(address, id, "stop"), {}), STOP_UNSAVED),
    setParams: (address, id, params) => control("PUT", routes.strategyParams(address, id), params),
    funding: () => call(fetchFn, "GET", routes.funding, undefined, readFunding, deadlineMs),
    declare: (accounts, cls, statement) =>
      call(fetchFn, "POST", routes.fundingAction("declare"), { accounts, class: cls, statement }, readJobs, deadlineMs),
    deposit: (accounts, amount, from) =>
      call(fetchFn, "POST", routes.fundingAction("deposit"), { accounts, amount, from }, readJobs, deadlineMs),
    claim: (accounts) => call(fetchFn, "POST", routes.fundingAction("claim"), { accounts }, readJobs, deadlineMs),
    withdraw: (accounts, amount) =>
      call(fetchFn, "POST", routes.fundingAction("withdraw"), amount === null ? { accounts } : { accounts, amount }, readJobs, deadlineMs),
  };
}
