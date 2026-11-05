/** The console's only door to the bot: every route, request and response check lives here. */
import { parseWith, readAccount, readErrorMessage, readFills, type Parsed } from "./parsers";
import type { AccountView, Fill } from "./types";

/** What a call came to. A refusal or an outage is a value the control shows, not an exception. */
export type Outcome<T> =
  | { readonly status: "ok"; readonly value: T }
  | { readonly status: "unauthenticated" }
  | { readonly status: "refused"; readonly message: string; readonly retryAfterSeconds: number | null }
  | { readonly status: "unavailable"; readonly message: string };

/** The calls the stores and pages make. */
export interface Api {
  session(): Promise<Outcome<null>>;
  login(password: string): Promise<Outcome<null>>;
  logout(): Promise<Outcome<null>>;
  account(address: string): Promise<Outcome<AccountView>>;
  fills(address: string): Promise<Outcome<readonly Fill[]>>;
  setAccountRunning(address: string, running: boolean): Promise<Outcome<null>>;
  flatten(address: string): Promise<Outcome<null>>;
  setStrategyRunning(address: string, strategyId: number, running: boolean): Promise<Outcome<null>>;
  setParams(address: string, strategyId: number, params: Readonly<Record<string, number>>): Promise<Outcome<null>>;
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

/** Turns a parse result into an outcome; a body that fails its check is an outage, never data. */
function checked<T>(parsed: Parsed<T>): Outcome<T> {
  return parsed.status === "ok"
    ? { status: "ok", value: parsed.value }
    : { status: "unavailable", message: `The bot sent an answer this console cannot read (${parsed.message}).` };
}

/** One request; `read` checks the success body. */
async function call<T>(
  fetchFn: Fetch,
  method: "GET" | "POST" | "PUT",
  path: string,
  body: unknown,
  read: (body: unknown) => T,
): Promise<Outcome<T>> {
  let response: Response;
  try {
    response = await fetchFn(path, {
      method,
      credentials: "same-origin",
      headers: body === undefined ? undefined : { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    return { status: "unavailable", message: "The bot did not answer." };
  }
  if (response.status === 401) {
    return { status: "unauthenticated" };
  }
  const json: unknown = await response.json().catch(() => null);
  if (response.ok) {
    return checked(parseWith(read, json));
  }
  const message = readErrorMessage(json) ?? `The bot refused the request (status ${String(response.status)}).`;
  return response.status >= 500
    ? { status: "unavailable", message }
    : { status: "refused", message, retryAfterSeconds: retryAfter(response) };
}

/** Reads no body: the session, login, logout and control answers are 204s with nothing the console uses. */
const nothing = (): null => null;

/** Builds the API client over `fetchFn` (the page's `fetch`; tests pass a stand-in). */
export function createApi(fetchFn: Fetch): Api {
  const control = (method: "POST" | "PUT", path: string, body: unknown) => call(fetchFn, method, path, body, nothing);
  return {
    session: () => call(fetchFn, "GET", routes.session, undefined, nothing),
    login: (password) => control("POST", routes.login, { password }),
    logout: () => control("POST", routes.logout, {}),
    account: (address) => call(fetchFn, "GET", routes.account(address), undefined, readAccount),
    fills: (address) => call(fetchFn, "GET", routes.fills(address), undefined, readFills),
    setAccountRunning: (address, running) => control("POST", routes.accountRun(address, running ? "start" : "stop"), {}),
    flatten: (address) => control("POST", routes.flatten(address), {}),
    setStrategyRunning: (address, id, running) => control("POST", routes.strategyRun(address, id, running ? "start" : "stop"), {}),
    setParams: (address, id, params) => control("PUT", routes.strategyParams(address, id), params),
  };
}
