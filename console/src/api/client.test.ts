/** Tests of the API client over a stand-in fetch. */
import { createApi, routes } from "./client";
import { detailBody } from "./fixtures";

/** A fetch that answers every call with `status`, `body` and `headers`, and records what it was asked. */
function fakeFetch(status: number, body: unknown, headers: Record<string, string> = {}) {
  const calls: { path: string; init: RequestInit | undefined }[] = [];
  const fetchFn = (path: string, init?: RequestInit) => {
    calls.push({ path, init });
    return Promise.resolve(new Response(JSON.stringify(body), { status, headers }));
  };
  return { fetchFn, calls };
}

describe("createApi", () => {
  it("sends a JSON POST with the cookie and no key or token of its own", async () => {
    const { fetchFn, calls } = fakeFetch(200, { seq: 2, account: detailBody() });
    const outcome = await createApi(fetchFn).flatten("0xabc");

    expect(outcome.status).toBe("ok");
    expect(calls).toEqual([
      {
        path: "/api/accounts/0xabc/flatten",
        init: { method: "POST", credentials: "same-origin", headers: { "Content-Type": "application/json" }, body: "{}" },
      },
    ]);
  });

  it("sends integer parameters as numbers in a PUT", async () => {
    const { fetchFn, calls } = fakeFetch(200, { seq: 2, account: detailBody() });
    await createApi(fetchFn).setParams("0xabc", "s 1", { fast: 5, width: "1.5" });

    expect(calls[0]?.path).toBe("/api/accounts/0xabc/strategies/s%201/params");
    expect(calls[0]?.init?.method).toBe("PUT");
    expect(calls[0]?.init?.body).toBe('{"params":{"fast":5,"width":"1.5"}}');
  });

  it("reports a 401 as an ended session", async () => {
    const { fetchFn } = fakeFetch(401, { error: "sign in" });
    expect(await createApi(fetchFn).accounts()).toEqual({ status: "unauthenticated" });
  });

  it("carries the bot's reason and Retry-After on a refusal", async () => {
    const { fetchFn } = fakeFetch(429, { error: "Too many login attempts." }, { "Retry-After": "30" });
    expect(await createApi(fetchFn).login("x")).toEqual({
      status: "refused", message: "Too many login attempts.", retryAfterSeconds: 30,
    });
  });

  it("falls back to the status when a refusal has no readable body", async () => {
    const fetchFn = () => Promise.resolve(new Response("<html>", { status: 400 }));
    expect(await createApi(fetchFn).flatten("0xabc")).toEqual({
      status: "refused", message: "The bot refused the request (status 400).", retryAfterSeconds: null,
    });
  });

  it("treats a server error and a dead connection as unavailable", async () => {
    const { fetchFn } = fakeFetch(503, { error: "engine unavailable" });
    expect(await createApi(fetchFn).accounts()).toEqual({ status: "unavailable", message: "engine unavailable" });
    const dead = () => Promise.reject(new TypeError("network"));
    expect(await createApi(dead).accounts()).toEqual({ status: "unavailable", message: "The bot did not answer." });
  });

  it("treats a success body that fails its check as unavailable, never as data", async () => {
    const { fetchFn } = fakeFetch(200, { seq: 1, accounts: [{ address: "0x1" }] });
    const outcome = await createApi(fetchFn).accounts();
    expect(outcome.status).toBe("unavailable");
  });
});

describe("routes", () => {
  it("encodes path parts so an id cannot change the route", () => {
    expect(routes.strategyRun("0xa", "../x", "stop")).toBe("/api/accounts/0xa/strategies/..%2Fx/stop");
  });
});
