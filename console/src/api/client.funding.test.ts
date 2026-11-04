/** Tests of the funding calls: what each sends, and that none can name a destination. */
import { createApi, routes } from "./client";
import { ALPHA, BETA, fundingBody } from "./funding-fixtures";

/** A fetch that answers every call with `status` and `body`, and records what it was asked. */
function fakeFetch(status: number, body: unknown) {
  const calls: { path: string; init: RequestInit | undefined }[] = [];
  const fetchFn = (path: string, init?: RequestInit) => {
    calls.push({ path, init });
    return Promise.resolve(new Response(JSON.stringify(body), { status }));
  };
  return { fetchFn, calls };
}

/** The JSON body a recorded call sent. */
function sentBody(call: { init: RequestInit | undefined } | undefined): unknown {
  const body = call?.init?.body;
  if (typeof body !== "string") {
    throw new Error("the call sent no JSON body");
  }
  const parsed: unknown = JSON.parse(body);
  return parsed;
}

const PROFESSIONAL = { class: "professional", statement: "I attest that I am a professional client." } as const;

describe("the funding calls", () => {
  it("reads the funding page with a GET and no body", async () => {
    const { fetchFn, calls } = fakeFetch(200, fundingBody());
    const outcome = await createApi(fetchFn).funding();
    expect(outcome.status).toBe("ok");
    expect(calls[0]?.path).toBe(routes.funding);
    expect(calls[0]?.init?.method).toBe("GET");
    expect(calls[0]?.init?.body).toBeUndefined();
  });

  it("sends a declaration with the class, the statement the user read, and the accounts", async () => {
    const { fetchFn, calls } = fakeFetch(200, fundingBody());
    await createApi(fetchFn).declare([ALPHA, BETA], PROFESSIONAL);
    expect(calls[0]?.path).toBe("/api/funding/declare");
    expect(calls[0]?.init?.method).toBe("POST");
    expect(sentBody(calls[0])).toEqual({
      accounts: [ALPHA, BETA], class: "professional", statement: "I attest that I am a professional client.",
    });
  });

  it("sends only accounts and an amount: no field names a destination", async () => {
    const { fetchFn, calls } = fakeFetch(200, fundingBody());
    const api = createApi(fetchFn);
    await api.deposit([ALPHA], "250.5");
    await api.claim([ALPHA, BETA]);
    await api.withdraw([BETA], "10");
    expect(calls.map((call) => [call.path, sentBody(call)])).toEqual([
      ["/api/funding/deposit", { accounts: [ALPHA], amount: "250.5" }],
      ["/api/funding/claim", { accounts: [ALPHA, BETA] }],
      ["/api/funding/withdraw", { accounts: [BETA], amount: "10" }],
    ]);
  });

  it("returns the bot's refusal as a value with its message", async () => {
    const { fetchFn } = fakeFetch(409, { error: "alpha is already depositing" });
    expect(await createApi(fetchFn).deposit([ALPHA], "5")).toEqual({
      status: "refused", message: "alpha is already depositing", retryAfterSeconds: null,
    });
  });

  it("treats a funding body that fails its check as an outage, not data", async () => {
    const { fetchFn } = fakeFetch(200, { ...fundingBody(), accounts: "none" });
    const outcome = await createApi(fetchFn).claim([ALPHA]);
    expect(outcome.status).toBe("unavailable");
  });
});
