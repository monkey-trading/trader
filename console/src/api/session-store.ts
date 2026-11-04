/** The login session: whether the bot's cookie is good. The console holds no token itself. */
import type { Api, Outcome } from "./client";

/** Whether a login is needed. */
export type SessionStatus = "checking" | "anonymous" | "authenticated";

/** What a login attempt came to, for the form to show. */
export type LoginResult =
  | { readonly status: "signed-in" }
  | { readonly status: "refused"; readonly message: string };

/** Holds the session status for components. */
export class SessionStore {
  private readonly api: Api;
  private status: SessionStatus = "checking";
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
    const outcome = await this.api.session();
    this.set(outcome.status === "ok" ? "authenticated" : "anonymous");
  }

  /** Signs in with `password`; a wrong or rate-limited attempt is a value for the form. */
  async login(password: string): Promise<LoginResult> {
    const outcome = await this.api.login(password);
    if (outcome.status === "ok") {
      this.set("authenticated");
      return { status: "signed-in" };
    }
    return { status: "refused", message: loginMessage(outcome) };
  }

  /** Ends the session at the bot, then here; the status changes even if the bot cannot be reached. */
  async logout(): Promise<void> {
    await this.api.logout();
    this.set("anonymous");
  }

  /** Marks the session gone, after the bot answered 401. */
  expire(): void {
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

/** The words for a login that did not succeed. */
function loginMessage(outcome: Exclude<Outcome<null>, { status: "ok" }>): string {
  switch (outcome.status) {
    case "unauthenticated":
      return "The password was not accepted.";
    case "refused":
      return outcome.retryAfterSeconds === null
        ? outcome.message
        : `${outcome.message} Try again in ${String(outcome.retryAfterSeconds)} seconds.`;
    case "unavailable":
      return outcome.message;
  }
}
