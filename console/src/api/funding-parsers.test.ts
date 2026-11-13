/** Tests of the funding parsers. */
import { readFunding, readJobs } from "./funding-parsers";
import { fundingAccountBody, fundingBody, jobBody } from "./funding-fixtures";
import { parseWith } from "./parsers";

describe("readFunding", () => {
  it("reads every account and job", () => {
    const funding = readFunding(
      fundingBody({
        accounts: [fundingAccountBody(undefined, { queued: [{ position: 2, remaining: "50" }], unpaid: [{ nonce: 1, amount: "5", authorized: true }] })],
        jobs: [jobBody(3, { state: "landed", txs: ["0xabc"], detail: "declared Professional" })],
      }),
    );
    if (!funding.enabled) {
      throw new Error("funding is enabled");
    }
    expect(funding.accounts[0]?.queued).toEqual([{ position: 2, remaining: "50" }]);
    expect(funding.accounts[0]?.unpaid).toEqual([{ nonce: 1, amount: "5", authorized: true }]);
    expect(funding.jobs[0]).toMatchObject({ id: 3, state: "landed", txs: ["0xabc"] });
  });

  it("reads an off funding as that alone", () => {
    expect(readFunding({ enabled: false })).toEqual({ enabled: false });
  });

  it("keeps an account that could not be read, with its reason and no figures", () => {
    const funding = readFunding(fundingBody({ accounts: [{ address: "0xabc", error: "no chain", busy: true }] }));
    if (!funding.enabled) {
      throw new Error("funding is enabled");
    }
    expect(funding.accounts[0]).toMatchObject({ address: "0xabc", error: "no chain", declared: null, balance: null, busy: true });
  });

  it.each([
    ["an unknown class", fundingBody({ accounts: [fundingAccountBody(undefined, { declared: "retail" })] })],
    ["an amount that is no decimal", fundingBody({ accounts: [fundingAccountBody(undefined, { wallet_usdc: "1e3" })] })],
    ["an unknown job state", fundingBody({ jobs: [jobBody(1, { state: "lost" })] })],
    ["a transaction that is no text", fundingBody({ jobs: [jobBody(1, { txs: [7] })] })],
    ["no statements", fundingBody({ statements: null })],
  ])("refuses %s", (_name, body) => {
    expect(parseWith(readFunding, body).status).toBe("invalid");
  });
});

describe("readJobs", () => {
  it("reads the jobs an action started", () => {
    expect(readJobs({ jobs: [jobBody(1), jobBody(2)] }).map((job) => job.id)).toEqual([1, 2]);
  });
});
