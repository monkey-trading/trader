/** Tests of the API client over a stand-in fetch. */
import { createApi, routes } from "./client";
import { fillBody, viewBody } from "./fixtures";

/** A fetch that answers every call with `status`, `body` and `headers`, and records what it was asked. */
function fakeFetch(status: number, body: unknown, headers: Record<string, string> = {}) {
  const calls: { path: string; init: RequestInit | undefined }[] = [];
  const fetchFn = (path: string, init?: RequestInit) => {
    calls.push({ path, init });
    return Promise.resolve(new Response(JSON.stringify(body), { status, headers }));
  };
  return { fetchFn, calls };
}

describe("createApi's deadlines", () => {
  /** A fetch that never answers until its signal aborts, then rejects as the browser's does. */
  const hanging = (path: string, init?: RequestInit) =>
    new Promise<Response>((_resolve, reject) => {
      init?.signal?.addEventListener("abort", () => { reject(new DOMException("aborted", "TimeoutError")); });
    });

  it("gives up on a call that never answers, saying a control may still act", async () => {
    const api = createApi(hanging, 20);
    expect(await api.account("0xabc")).toEqual({ status: "unavailable", message: "The bot did not answer in time." });
    expect(await api.setAccountRunning("0xabc", false)).toEqual({
      status: "unavailable", message: "The bot did not answer in time; it may still act on the request.",
    });
  });

  it("gives up on an answer whose body never finishes", async () => {
    const stalled = (path: string, init?: RequestInit) => {
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          init?.signal?.addEventListener("abort", () => { controller.error(new DOMException("aborted", "TimeoutError")); });
        },
      });
      return Promise.resolve(new Response(body, { status: 200 }));
    };
    expect(await createApi(stalled, 20).account("0xabc")).toEqual({ status: "unavailable", message: "The bot did not answer in time." });
  });
});

describe("createApi", () => {
  it("sends a JSON POST with the cookie, and a 204 is success", async () => {
    const fetchFn = (path: string, init?: RequestInit) => {
      calls.push({ path, init });
      return Promise.resolve(new Response(null, { status: 204 }));
    };
    const calls: { path: string; init: RequestInit | undefined }[] = [];
    const outcome = await createApi(fetchFn).flatten("0xabc");

    expect(outcome).toEqual({ status: "ok", value: null });
    expect(calls).toEqual([
      {
        path: "/api/accounts/0xabc/flatten",
        init: {
          method: "POST", credentials: "same-origin", headers: { "Content-Type": "application/json" }, body: "{}",
          signal: expect.any(AbortSignal) as unknown,
        },
      },
    ]);
  });

  it("puts the periods as the body of a PUT, with the strategy's numeric id in the path", async () => {
    const { fetchFn, calls } = fakeFetch(204, null);
    await createApi(fetchFn).setParams("0xabc", 0, { fast: 5, slow: 20 });

    expect(calls[0]?.path).toBe("/api/accounts/0xabc/strategies/0/params");
    expect(calls[0]?.init?.method).toBe("PUT");
    expect(calls[0]?.init?.body).toBe('{"fast":5,"slow":20}');
  });

  it("reads an account and its fills through their parsers", async () => {
    expect((await createApi(fakeFetch(200, viewBody()).fetchFn).account("0xabc")).status).toBe("ok");
    const fills = await createApi(fakeFetch(200, { fills: [fillBody()] }).fetchFn).fills("0xabc");
    expect(fills.status === "ok" && fills.value.length).toBe(1);
  });

  it("reports a 401 as an ended session", async () => {
    const { fetchFn } = fakeFetch(401, { error: "log in first", code: "session_required" });
    expect(await createApi(fetchFn).session()).toEqual({ status: "unauthenticated", code: "session_required" });
  });

  it("carries the bot's reason and Retry-After on a refusal", async () => {
    const { fetchFn } = fakeFetch(429, { error: "too many attempts; try again later", code: "rate_limited" }, { "Retry-After": "30" });
    expect(await createApi(fetchFn).login("x")).toEqual({
      status: "refused", message: "too many attempts; try again later", code: "rate_limited", retryAfterSeconds: 30,
    });
  });

  it("falls back to the status when a refusal has no readable body", async () => {
    const fetchFn = () => Promise.resolve(new Response("<html>", { status: 422 }));
    expect(await createApi(fetchFn).flatten("0xabc")).toEqual({
      status: "refused", message: "The bot refused the request (status 422).", code: null, retryAfterSeconds: null,
    });
  });

  it("says in words when a request was too slow or too large, whatever the body", async () => {
    const slow = () => Promise.resolve(new Response("", { status: 408 }));
    const large = () => Promise.resolve(new Response("payload too large", { status: 413 }));
    expect(await createApi(slow).setParams("0xabc", 0, { fast: 5 })).toEqual({
      status: "refused", message: "The request took too long to send, so the bot dropped it.", code: null, retryAfterSeconds: null,
    });
    expect(await createApi(large).login("x")).toEqual({
      status: "refused", message: "The request was too large for the bot to take.", code: null, retryAfterSeconds: null,
    });
  });

  it("carries a refusal's message and code as the bot gives them", async () => {
    const refuse = (error: string, code: string) => () =>
      Promise.resolve(new Response(JSON.stringify({ error, code }), { status: 422 }));
    expect(await createApi(refuse("the account is flattening", "flatten_in_progress")).setAccountRunning("0xabc", true)).toEqual({
      status: "refused", message: "the account is flattening", code: "flatten_in_progress", retryAfterSeconds: null,
    });
    expect(await createApi(refuse("fast is outside 2 to 50", "out_of_bounds")).setParams("0xabc", 0, { kind: "sma_crossover" })).toMatchObject({
      status: "refused", code: "out_of_bounds",
    });
    expect(await createApi(refuse("kind does not match", "kind_mismatch")).setParams("0xabc", 0, { kind: "macd" })).toMatchObject({
      code: "kind_mismatch",
    });
  });

  it("reads a flatten with the code flattened_stop_unsaved as done, with the bot's message as the warning", async () => {
    const message = "words the console does not match on";
    const { fetchFn } = fakeFetch(500, { error: message, code: "flattened_stop_unsaved" });
    expect(await createApi(fetchFn).flatten("0xabc")).toEqual({ status: "ok", value: { warning: message } });
  });

  it("keeps any other 500 of a flatten, and that code on any other control, a failure, whatever the words", async () => {
    const { fetchFn } = fakeFetch(500, { error: "the account is flattened; the stop holds, but was not saved" });
    expect(await createApi(fetchFn).flatten("0xabc")).toEqual({
      status: "unavailable", message: "the account is flattened; the stop holds, but was not saved", code: null,
    });
    const same = fakeFetch(500, { error: "x", code: "flattened_stop_unsaved" });
    expect((await createApi(same.fetchFn).setAccountRunning("0xabc", false)).status).toBe("unavailable");
  });

  it("treats a server error and a dead connection as unavailable", async () => {
    const { fetchFn } = fakeFetch(504, { error: "the account has not answered yet; it may still act on the request" });
    expect((await createApi(fetchFn).flatten("0xabc")).status).toBe("unavailable");
    const dead = () => Promise.reject(new TypeError("network"));
    expect(await createApi(dead).session()).toEqual({ status: "unavailable", message: "The bot did not answer." });
  });

  it("treats a success body that fails its check as unavailable, never as data", async () => {
    const { fetchFn } = fakeFetch(200, { address: "0x1" });
    expect((await createApi(fetchFn).account("0x1")).status).toBe("unavailable");
  });
});

describe("routes", () => {
  it("encodes the address so it cannot change the route", () => {
    expect(routes.accountRun("0xa/../b", "stop")).toBe("/api/accounts/0xa%2F..%2Fb/stop");
  });
});
