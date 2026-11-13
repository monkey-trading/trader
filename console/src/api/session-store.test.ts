/** Tests of the session store. */
import type { Api, Outcome } from "./client";
import { SessionStore } from "./session-store";

const ok: Outcome<null> = { status: "ok", value: null };

function storeWith(api: Partial<Api>) {
  return new SessionStore(api as Api);
}

describe("SessionStore", () => {
  it("starts unknown, then follows the bot's answer to the cookie check", async () => {
    const store = storeWith({ session: () => Promise.resolve(ok) });
    expect(store.getSnapshot()).toBe("checking");
    await store.check();
    expect(store.getSnapshot()).toBe("authenticated");
    const anonymous = storeWith({ session: () => Promise.resolve({ status: "unauthenticated" }) });
    await anonymous.check();
    expect(anonymous.getSnapshot()).toBe("anonymous");
  });

  it("signs in on success and says why on a refusal, staying signed out", async () => {
    const outcomes: Outcome<null>[] = [
      { status: "unauthenticated" },
      { status: "unauthenticated", code: "wrong_password" },
      { status: "refused", message: "Too many login attempts.", retryAfterSeconds: 30 },
      ok,
    ];
    const store = storeWith({ login: () => Promise.resolve(outcomes.shift() ?? ok) });
    expect(await store.login("a")).toEqual({ status: "refused", message: "The password was not accepted." });
    expect(await store.login("a")).toEqual({ status: "refused", message: "Wrong password. Try again." });
    expect(await store.login("a")).toEqual({
      status: "refused", message: "Too many login attempts. Try again in 30 seconds.",
    });
    expect(store.getSnapshot()).toBe("checking");
    expect(await store.login("a")).toEqual({ status: "signed-in" });
    expect(store.getSnapshot()).toBe("authenticated");
  });

  it("stays signed in when the bot did not end the session, and says why", async () => {
    const store = storeWith({
      login: () => Promise.resolve(ok),
      logout: () => Promise.resolve({ status: "unavailable", message: "down" }),
    });
    await store.login("a");
    let heard = 0;
    store.subscribe(() => { heard += 1; });
    expect(await store.logout()).toEqual({ status: "failed", message: "down" });
    expect(store.getSnapshot()).toBe("authenticated");
    expect(heard).toBe(0);
  });

  it("signs out when the bot ended the session or says it is already gone, and tells subscribers once", async () => {
    for (const outcome of [ok, { status: "unauthenticated", code: null } as const]) {
      const store = storeWith({ login: () => Promise.resolve(ok), logout: () => Promise.resolve(outcome) });
      await store.login("a");
      let heard = 0;
      store.subscribe(() => { heard += 1; });
      expect(await store.logout()).toEqual({ status: "signed-out" });
      store.expire();
      expect(store.getSnapshot()).toBe("anonymous");
      expect(heard).toBe(1);
    }
  });
});
