/** The login session: whether the bot's cookie is good. The console holds no token itself. */
import type { Api, Outcome } from "./client";

/** Whether a login is needed. */
export type SessionStatus = "checking" | "anonymous" | "authenticated" | "unreachable";

/** What a login attempt came to, for the form to show. */
export type LoginResult =
  | { readonly status: "signed-in" }
  | { readonly status: "refused"; readonly message: string };

/** What a sign out came to, for the control to show. */
export type LogoutResult =
  | { readonly status: "signed-out" }
  | { readonly status: "failed"; readonly message: string };

/** Holds the session status for components. */
export class SessionStore {
  private readonly api: Api;
  private status: SessionStatus = "checking";
  /** Moves on at each login, sign out and expiry, so a check that began before one is not taken as its news. */
  private transitions = 0;
  private readonly listeners = new Set<() => void>();

  constructor(api: Api) {
    this.api = api;
  }

  /** Subscribes `listener` to status changes; returns the unsubscriber. */
  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  /** The current status. */
  readonly getSnapshot = (): SessionStatus => this.status;

  /** Asks the bot whether the cookie is still good. */
  async check(): Promise<void> {
    const before = this.transitions;
    const outcome = await this.api.session();
    if (this.transitions === before) {
      // A bot that did not answer says nothing of the session: the login page would hide an outage.
      this.set(outcome.status === "ok" ? "authenticated" : outcome.status === "unavailable" ? "unreachable" : "anonymous");
    }
  }

  /** Signs in with `password`; a wrong or rate-limited attempt is a value for the form. */
  async login(password: string): Promise<LoginResult> {
    const outcome = await this.api.login(password);
    if (outcome.status === "ok") {
      this.transitions += 1;
      this.set("authenticated");
      return { status: "signed-in" };
    }
    return { status: "refused", message: loginMessage(outcome) };
  }

  /**
   * Ends the session at the bot, then here. A bot that could not be reached or refused still holds the session, so
   * the console stays signed in and says so: showing the login page would hide a cookie that is good for hours.
   */
  async logout(): Promise<LogoutResult> {
    const outcome = await this.api.logout();
    if (outcome.status === "ok" || outcome.status === "unauthenticated") {
      this.transitions += 1;
      this.set("anonymous");
      return { status: "signed-out" };
    }
    return { status: "failed", message: outcome.message };
  }

  /** How many times the session has changed hands; a request carries it to say which session it was made in. */
  get generation(): number {
    return this.transitions;
  }

  /** Marks the session gone, after the bot answered 401 to a request made in session `generation`, if it still is. */
  expire(generation: number = this.transitions): void {
    if (generation !== this.transitions) {
      return;
    }
    this.transitions += 1;
    this.set("anonymous");
  }

  private set(status: SessionStatus): void {
    if (status !== this.status) {
      this.status = status;
      for (const listener of this.listeners) {
        listener();
      }
    }
  }
}

/** The bot's code for a login with the wrong password. */
const WRONG_PASSWORD = "wrong_password";

/** The words for a login that did not succeed. */
function loginMessage(outcome: Exclude<Outcome<null>, { status: "ok" }>): string {
  switch (outcome.status) {
    case "unauthenticated":
      return outcome.code === WRONG_PASSWORD ? "Wrong password. Try again." : "The password was not accepted.";
    case "refused":
      return outcome.retryAfterSeconds === null
        ? outcome.message
        : `${outcome.message} Try again in ${String(outcome.retryAfterSeconds)} seconds.`;
    case "unavailable":
      return outcome.message;
  }
}
