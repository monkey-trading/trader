/** Tests of the funding parser: a valid body, and each way a body can be wrong. */
import { describe, expect, it } from "vitest";

import { ALPHA, BETA, fundingAccountBody, fundingBody, operationBody } from "./funding-fixtures";
import { readFunding } from "./funding-parsers";
import { ShapeError } from "@/lib/shape";

describe("readFunding", () => {
  it("reads accounts, declarations, the gas payer and operations whole", () => {
    const read = readFunding(
      fundingBody({ gasPayer: BETA, operations: [operationBody({ state: "failed", reason: "ERC20: insufficient allowance" })] }),
    );
    expect(read.accounts[1]).toEqual({
      address: BETA, name: "beta", declared: "professional", walletUsdc: "1000.5", walletEth: "0.25", balance: "250",
      unclaimedMonkey: "7.5", queued: [], unpaid: [],
    });
    expect(read.declarations).toEqual([
      { class: "professional", statement: "I attest that I am a professional client." },
      { class: "eligible_client", statement: "I attest that I am an eligible client." },
    ]);
    expect(read.gasPayer).toBe(BETA);
    expect(read.operations).toEqual([
      {
        id: "op-1", kind: "deposit", address: ALPHA, state: "failed", stage: "Waiting for the credit in the log",
        txHash: `0x${"ab".repeat(32)}`, reason: "ERC20: insufficient allowance",
      },
    ]);
  });

  it("reads queued withdrawals and unpaid approvals", () => {
    const read = readFunding(
      fundingBody({
        accounts: [
          fundingAccountBody({
            queued: [{ position: 3, amount: "100", remaining: "40.25" }],
            unpaid: [{ nonce: 2, amount: "60" }],
          }),
        ],
      }),
    );
    expect(read.accounts[0]?.queued).toEqual([{ position: 3, amount: "100", remaining: "40.25" }]);
    expect(read.accounts[0]?.unpaid).toEqual([{ nonce: 2, amount: "60" }]);
  });

  it("drops fields it does not know", () => {
    const read = readFunding(fundingBody({ extra: 1, accounts: [fundingAccountBody({ privateKey: "0x01" })] }));
    expect(Object.keys(read.accounts[0] ?? {})).not.toContain("privateKey");
    expect(Object.keys(read)).not.toContain("extra");
  });

  it.each([
    ["a body that is not an object", []],
    ["an unknown declaration class", fundingBody({ declarations: [{ class: "retail", statement: "x" }] })],
    ["a balance that is not a decimal", fundingBody({ accounts: [fundingAccountBody({ balance: "1e3" })] })],
    ["a wallet amount sent as a number", fundingBody({ accounts: [fundingAccountBody({ walletUsdc: 5 })] })],
    ["an account declared as an unknown class", fundingBody({ accounts: [fundingAccountBody({ declared: "retail" })] })],
    ["an unknown operation state", fundingBody({ operations: [operationBody({ state: "done" })] })],
    ["an operation without an address", fundingBody({ operations: [{ ...operationBody(), address: undefined }] })],
    ["a missing accounts list", { ...fundingBody(), accounts: undefined }],
  ])("refuses %s", (_name, body) => {
    expect(() => readFunding(body)).toThrow(ShapeError);
  });
});
