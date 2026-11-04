/** Tests of the funding rules: typed amounts, who an action applies to, and what a confirmation covers. */
import { readFunding } from "@/api/funding-parsers";
import { ALPHA, BETA, fundingAccountBody, fundingBody, operationBody } from "@/api/funding-fixtures";

import {
  confirmationKey, isBusy, operationsOfKind, OPERATIONS_SHOWN, operationStatus, parseUsdcAmount, selectedAccounts,
  undeclared, withUnclaimed, withdrawalSummary,
} from "./funding";

/** The first element of a fixture list; its absence is a defect in the test. */
function first<T>(list: readonly T[]): T {
  const item = list[0];
  if (item === undefined) {
    throw new Error("the fixture list is empty");
  }
  return item;
}

const read = readFunding(fundingBody());
const [alpha, beta] = read.accounts;
if (alpha === undefined || beta === undefined) {
  throw new Error("the fixture has two accounts");
}

describe("parseUsdcAmount", () => {
  it.each(["250", "12.5", "0.000001", " 7 ", "1000000000"])("accepts %j", (input) => {
    expect(parseUsdcAmount(input)).toEqual({ status: "ok", value: input.trim() });
  });

  it.each([
    ["", "Enter an amount in USDC."],
    ["   ", "Enter an amount in USDC."],
    ["abc", "Enter a plain positive number such as 250 or 12.5."],
    ["-5", "Enter a plain positive number such as 250 or 12.5."],
    ["1e3", "Enter a plain positive number such as 250 or 12.5."],
    ["1,000", "Enter a plain positive number such as 250 or 12.5."],
    ["0.0000001", "USDC has at most 6 decimal places."],
    ["0", "The amount must be above zero."],
    ["0.000000", "The amount must be above zero."],
  ])("refuses %j", (input, message) => {
    expect(parseUsdcAmount(input)).toEqual({ status: "invalid", message });
  });
});

describe("who an action applies to", () => {
  it("selects the listed accounts in the list's order", () => {
    expect(selectedAccounts(read.accounts, new Set([BETA, ALPHA, "0xgone"]))).toEqual([alpha, beta]);
  });

  it("finds the accounts that have not declared and those with MONKEY to claim", () => {
    expect(undeclared(read.accounts)).toEqual([alpha]);
    expect(withUnclaimed(read.accounts)).toEqual([beta]);
  });

  it("treats an unclaimed amount of exactly zero as nothing to claim, at any precision", () => {
    const dust = readFunding(fundingBody({ accounts: [fundingAccountBody({ unclaimedMonkey: "0.000000000000000001" })] }));
    expect(withUnclaimed(dust.accounts)).toHaveLength(1);
    const none = readFunding(fundingBody({ accounts: [fundingAccountBody({ unclaimedMonkey: "0.000" })] }));
    expect(withUnclaimed(none.accounts)).toEqual([]);
  });
});

describe("confirmationKey", () => {
  it("names the class and the accounts, whatever their order", () => {
    expect(confirmationKey("professional", [alpha, beta])).toBe(confirmationKey("professional", [beta, alpha]));
  });

  it("differs when the class or the accounts change", () => {
    const base = confirmationKey("professional", [alpha]);
    expect(confirmationKey("eligible_client", [alpha])).not.toBe(base);
    expect(confirmationKey("professional", [alpha, beta])).not.toBe(base);
    expect(confirmationKey("professional", [])).not.toBe(base);
  });
});

describe("operations", () => {
  const operations = readFunding(
    fundingBody({
      operations: [
        operationBody({ id: "a", kind: "deposit" }),
        operationBody({ id: "b", kind: "claim", state: "landed" }),
        operationBody({ id: "c", kind: "deposit", address: BETA, state: "landed" }),
      ],
    }),
  ).operations;

  it("lists the operations of one kind in the order given", () => {
    expect(operationsOfKind(operations, "deposit").map((operation) => operation.id)).toEqual(["a", "c"]);
  });

  it("lists no more than the page shows", () => {
    const many = Array.from({ length: OPERATIONS_SHOWN + 5 }, (_, index) => ({ ...first(operations), id: String(index) }));
    expect(operationsOfKind(many, "deposit")).toHaveLength(OPERATIONS_SHOWN);
  });

  it("is busy only for a pending operation of that kind and account", () => {
    expect(isBusy(operations, "deposit", ALPHA)).toBe(true);
    expect(isBusy(operations, "deposit", BETA)).toBe(false);
    expect(isBusy(operations, "claim", ALPHA)).toBe(false);
  });

  it("words each state, with a failure's reason", () => {
    expect(operationStatus(first(operations))).toBe("Waiting for the credit in the log…");
    expect(operationStatus(first(operations.filter((operation) => operation.state === "landed")))).toBe("Landed");
    const failed = readFunding(fundingBody({ operations: [operationBody({ state: "failed", reason: "Vault: below minimum" })] }));
    expect(operationStatus(first(failed.operations))).toBe("Failed: Vault: below minimum");
    const bare = readFunding(fundingBody({ operations: [operationBody({ state: "failed", reason: null })] }));
    expect(operationStatus(first(bare.operations))).toBe("Failed: no reason given");
  });
});

describe("withdrawalSummary", () => {
  it("is a dash with nothing queued or unpaid", () => {
    expect(withdrawalSummary(alpha)).toBe("—");
  });

  it("sums queued remainders and unpaid approvals exactly", () => {
    const account = first(readFunding(
      fundingBody({
        accounts: [
          fundingAccountBody({
            queued: [
              { position: 2, amount: "100", remaining: "40.000001" },
              { position: 5, amount: "10", remaining: "10" },
            ],
            unpaid: [{ nonce: 3, amount: "60.5" }],
          }),
        ],
      }),
    ).accounts);
    expect(withdrawalSummary(account)).toBe("2 queued (50.000001 USDC unapproved); 1 approved, unpaid (60.5 USDC)");
  });
});
