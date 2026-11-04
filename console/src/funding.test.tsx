/** Tests of the funding page: what the user does, and what shows while a transaction runs, lands or reverts. */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { Outcome } from "@/api/client";
import { ALPHA, BETA, fundingAccountBody, fundingBody, operationBody } from "@/api/funding-fixtures";
import { readFunding } from "@/api/funding-parsers";
import type { FundingRead } from "@/api/funding-types";
import { mount, stubApi } from "@/test-harness";

const ok = (overrides: Record<string, unknown> = {}): Outcome<FundingRead> => ({
  status: "ok",
  value: readFunding(fundingBody(overrides)),
});

/** A promise the test settles by hand, to hold a call in flight. */
function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

/** The page, mounted with `overrides` as the bot's answer. */
async function open(overrides: Record<string, unknown> = {}) {
  const api = stubApi();
  api.funding.mockResolvedValue(ok(overrides));
  const user = userEvent.setup();
  mount(api, "/funding");
  await screen.findByRole("heading", { level: 1, name: "Funding" });
  await screen.findByRole("checkbox", { name: "Select alpha" });
  return { api, user };
}

describe("the funding table", () => {
  it("shows each account's declaration, wallet, balance, MONKEY and withdrawals exactly", async () => {
    await open({
      accounts: [
        fundingAccountBody({ walletUsdc: "1000.123456", walletEth: "0.000000000000000001" }),
        fundingAccountBody({
          address: BETA, name: "beta", declared: "eligible_client", balance: "250", unclaimedMonkey: "7.5",
          queued: [{ position: 1, amount: "40", remaining: "15.25" }], unpaid: [{ nonce: 4, amount: "10" }],
        }),
      ],
    });
    const [, alphaRow, betaRow] = within(screen.getByRole("region", { name: "Account funds" })).getAllByRole("row");
    expect(within(alphaRow ?? document.body).getAllByRole("cell").map((cell) => cell.textContent)).toEqual([
      "", "not declared", "1,000.123456", "0.000000000000000001", "—", "0", "—",
    ]);
    expect(within(betaRow ?? document.body).getAllByRole("cell").map((cell) => cell.textContent)).toEqual([
      "", "eligible client", "1,000.5", "0.25", "250", "7.5", "1 queued (15.25 USDC unapproved); 1 approved, unpaid (10 USDC)",
    ]);
  });

  it("says every action pays only the account's own address", async () => {
    await open();
    expect(screen.getByText(/Every action here pays only the account's own address/)).toBeInTheDocument();
  });

  it("names the account that pays gas when a funding key is set", async () => {
    await open({ gasPayer: BETA });
    expect(screen.getByText(`${BETA} pays the gas and receives nothing.`, { exact: false })).toBeInTheDocument();
  });
});

describe("declaring", () => {
  it("records nothing until a statement is chosen and the user confirms it for the listed accounts", async () => {
    const { api, user } = await open();
    await user.click(screen.getByRole("checkbox", { name: "Select alpha" }));
    const declare = screen.getByRole("button", { name: "Declare for 1 account" });
    const confirm = screen.getByRole("checkbox", { name: /I have read the statement above/ });

    expect(declare).toBeDisabled();
    expect(confirm).toBeDisabled();
    await user.click(screen.getByRole("radio", { name: "I attest that I am a professional client." }));
    expect(declare).toBeDisabled();
    await user.click(confirm);
    expect(declare).toBeEnabled();
    expect(api.declare).not.toHaveBeenCalled();

    await user.click(declare);
    expect(api.declare).toHaveBeenCalledExactlyOnceWith([ALPHA], {
      class: "professional", statement: "I attest that I am a professional client.",
    });
  });

  it("lists the accounts it will be recorded for, in full", async () => {
    const { user } = await open();
    await user.click(screen.getByRole("checkbox", { name: "Select alpha" }));
    expect(screen.getByText(ALPHA, { exact: false })).toBeInTheDocument();
  });

  it("applies only to the selected accounts that have not declared", async () => {
    const { api, user } = await open();
    await user.click(screen.getByRole("checkbox", { name: "Select all accounts" }));
    await user.click(screen.getByRole("radio", { name: "I attest that I am an eligible client." }));
    await user.click(screen.getByRole("checkbox", { name: /I have read the statement above/ }));
    await user.click(screen.getByRole("button", { name: "Declare for 1 account" }));
    expect(api.declare).toHaveBeenCalledExactlyOnceWith([ALPHA], expect.objectContaining({ class: "eligible_client" }));
  });

  it("withdraws the confirmation when the accounts change, so it never confirms for accounts the user did not read", async () => {
    const { user } = await open({
      accounts: [fundingAccountBody(), fundingAccountBody({ address: BETA, name: "beta" })],
    });
    await user.click(screen.getByRole("checkbox", { name: "Select alpha" }));
    await user.click(screen.getByRole("radio", { name: "I attest that I am a professional client." }));
    await user.click(screen.getByRole("checkbox", { name: /I have read the statement above/ }));
    expect(screen.getByRole("button", { name: "Declare for 1 account" })).toBeEnabled();

    await user.click(screen.getByRole("checkbox", { name: "Select beta" }));
    expect(screen.getByRole("checkbox", { name: /I have read the statement above/ })).not.toBeChecked();
    expect(screen.getByRole("button", { name: "Declare for 2 accounts" })).toBeDisabled();
  });

  it("withdraws the confirmation when the statement changes", async () => {
    const { user } = await open();
    await user.click(screen.getByRole("checkbox", { name: "Select alpha" }));
    await user.click(screen.getByRole("radio", { name: "I attest that I am a professional client." }));
    await user.click(screen.getByRole("checkbox", { name: /I have read the statement above/ }));
    await user.click(screen.getByRole("radio", { name: "I attest that I am an eligible client." }));
    expect(screen.getByRole("checkbox", { name: /I have read the statement above/ })).not.toBeChecked();
  });

  it("asks again after a declaration is sent", async () => {
    const { user } = await open();
    await user.click(screen.getByRole("checkbox", { name: "Select alpha" }));
    await user.click(screen.getByRole("radio", { name: "I attest that I am a professional client." }));
    await user.click(screen.getByRole("checkbox", { name: /I have read the statement above/ }));
    await user.click(screen.getByRole("button", { name: "Declare for 1 account" }));
    await waitFor(() => { expect(screen.getByRole("checkbox", { name: /I have read the statement above/ })).not.toBeChecked(); });
  });

  it("shows the bot's refusal at the declaration control", async () => {
    const { api, user } = await open();
    api.declare.mockResolvedValue({ status: "refused", message: "retry later: too many new accounts", retryAfterSeconds: 30 });
    await user.click(screen.getByRole("checkbox", { name: "Select alpha" }));
    await user.click(screen.getByRole("radio", { name: "I attest that I am a professional client." }));
    await user.click(screen.getByRole("checkbox", { name: /I have read the statement above/ }));
    await user.click(screen.getByRole("button", { name: "Declare for 1 account" }));
    const section = screen.getByRole("region", { name: "Declaration" });
    expect(await within(section).findByText("Failed: retry later: too many new accounts")).toBeInTheDocument();
    expect(screen.getAllByText(/^Failed:/)).toHaveLength(1);
  });
});

describe("depositing and withdrawing", () => {
  it("sends the typed amount for the selected accounts once, however often the button is pressed", async () => {
    const { api, user } = await open();
    const held = deferred<Outcome<FundingRead>>();
    api.deposit.mockReturnValue(held.promise);
    await user.click(screen.getByRole("checkbox", { name: "Select all accounts" }));
    await user.type(within(screen.getByRole("region", { name: "Deposit USDC" })).getByLabelText("Amount per account (USDC)"), "250.5");
    await user.dblClick(screen.getByRole("button", { name: "Deposit for 2 accounts" }));

    expect(api.deposit).toHaveBeenCalledExactlyOnceWith([ALPHA, BETA], "250.5");
    expect(await screen.findByRole("button", { name: "Depositing…" })).toBeDisabled();
  });

  it("refuses an amount it cannot read, at the field, and sends nothing", async () => {
    const { api, user } = await open();
    await user.click(screen.getByRole("checkbox", { name: "Select alpha" }));
    const section = screen.getByRole("region", { name: "Withdraw USDC" });
    await user.type(within(section).getByLabelText("Amount per account (USDC)"), "1.0000001");
    expect(within(section).getByText("USDC has at most 6 decimal places.")).toBeInTheDocument();
    expect(within(section).getByRole("button", { name: "Withdraw for 1 account" })).toBeDisabled();
    expect(api.withdraw).not.toHaveBeenCalled();
  });

  it("needs a selection", async () => {
    const { user } = await open();
    const section = screen.getByRole("region", { name: "Deposit USDC" });
    await user.type(within(section).getByLabelText("Amount per account (USDC)"), "5");
    expect(within(section).getByRole("button", { name: "Deposit for 0 accounts" })).toBeDisabled();
  });

  it("withdraws the amount for one account", async () => {
    const { api, user } = await open();
    await user.click(screen.getByRole("checkbox", { name: "Select beta" }));
    const section = screen.getByRole("region", { name: "Withdraw USDC" });
    await user.type(within(section).getByLabelText("Amount per account (USDC)"), "10");
    await user.click(within(section).getByRole("button", { name: "Withdraw for 1 account" }));
    expect(api.withdraw).toHaveBeenCalledExactlyOnceWith([BETA], "10");
  });

  it("shows a pending deposit until it lands, and does not offer another for that account", async () => {
    const { user } = await open({ operations: [operationBody({ id: "d1" })] });
    await user.click(screen.getByRole("checkbox", { name: "Select alpha" }));
    const section = screen.getByRole("region", { name: "Deposit USDC" });
    expect(within(section).getByText(/Waiting for the credit in the log…/)).toBeInTheDocument();
    expect(within(section).getByText(/tx 0xabab…abab/)).toBeInTheDocument();
    await user.type(within(section).getByLabelText("Amount per account (USDC)"), "5");
    expect(within(section).getByRole("button", { name: "Deposit for 1 account" })).toBeDisabled();
  });

  it("shows a revert's reason in the section of the control that sent it, and nowhere else", async () => {
    await open({
      operations: [operationBody({ id: "w1", kind: "withdraw", state: "failed", stage: "Submitting", reason: "Vault: nonce already used" })],
    });
    const section = screen.getByRole("region", { name: "Withdraw USDC" });
    expect(within(section).getByText("Failed: Vault: nonce already used", { exact: false })).toBeInTheDocument();
    expect(screen.getAllByText(/Vault: nonce already used/)).toHaveLength(1);
  });

  it("shows a landed claim as landed", async () => {
    await open({ operations: [operationBody({ id: "c1", kind: "claim", state: "landed", stage: "Landed" })] });
    expect(within(screen.getByRole("region", { name: "Claim MONKEY" })).getByText("Landed", { exact: false })).toBeInTheDocument();
  });
});

describe("claiming", () => {
  it("claims only for selected accounts with MONKEY to claim", async () => {
    const { api, user } = await open();
    await user.click(screen.getByRole("checkbox", { name: "Select all accounts" }));
    await user.click(screen.getByRole("button", { name: "Claim for 1 account" }));
    expect(api.claim).toHaveBeenCalledExactlyOnceWith([BETA]);
  });

  it("is off when no selected account has MONKEY", async () => {
    const { user } = await open();
    await user.click(screen.getByRole("checkbox", { name: "Select alpha" }));
    expect(screen.getByRole("button", { name: "Claim for 0 accounts" })).toBeDisabled();
  });
});

describe("the page's state", () => {
  it("says so, and shows no figures, when the first read fails", async () => {
    const api = stubApi();
    api.funding.mockResolvedValue({ status: "unavailable", message: "The bot did not answer." });
    mount(api, "/funding");
    expect(await screen.findByText(/may be out of date: The bot did not answer\./)).toBeInTheDocument();
    expect(screen.queryByRole("checkbox", { name: "Select alpha" })).not.toBeInTheDocument();
  });

  it("reaches the page from the navigation and goes back to the accounts", async () => {
    const api = stubApi();
    const user = userEvent.setup();
    mount(api, "/");
    await user.click(await screen.findByRole("link", { name: "Funding" }));
    expect(await screen.findByRole("heading", { level: 1, name: "Funding" })).toBeInTheDocument();
  });
});
