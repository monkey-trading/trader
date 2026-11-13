/** Tests of the funding page: what it shows, and what each action sends for the accounts selected. */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { ADDRESS } from "@/api/fixtures";
import { ELIGIBLE, fundingBody, jobBody, OTHER, PROFESSIONAL } from "@/api/funding-fixtures";
import { readFunding, readJobs } from "@/api/funding-parsers";
import { mount, stubApi } from "@/test-harness";

function funded(overrides: Record<string, unknown> = {}) {
  const api = stubApi();
  api.funding.mockResolvedValue({ status: "ok", value: readFunding(fundingBody(overrides)) });
  return api;
}

/** Ticks the checkbox of `address`. */
async function select(user: ReturnType<typeof userEvent.setup>, address: string) {
  await user.click(await screen.findByRole("checkbox", { name: `Select ${address}` }));
}

describe("the funding page", () => {
  it("shows each account's declaration, wallet, balance and withdrawals", async () => {
    const [one, two] = fundingBody().accounts as Record<string, unknown>[];
    const api = funded({
      accounts: [
        { ...one, unclaimed: "12.5", unclaimed_newest: "20", queued: [{ position: 3, remaining: "40" }], payouts_loaded: false },
        two,
      ],
    });
    mount(api, "/funding");

    const table = await screen.findByRole("region", { name: "Funding of each account" });
    const first = within(table).getByRole("row", { name: /0x1111…1111/ });
    expect(within(first).getByText("Not declared")).toBeInTheDocument();
    expect(within(first).getByText("Never credited")).toBeInTheDocument();
    expect(within(first).getByText("12.5 (20 next)")).toBeInTheDocument();
    expect(within(first).getByText("40 queued at 3 (more may still load)")).toBeInTheDocument();
    const second = within(table).getByRole("row", { name: /0x2222…2222/ });
    expect(within(second).getByText("professional")).toBeInTheDocument();
    expect(within(second).getByText("1,000")).toBeInTheDocument();
  });

  it("says funding is off when the bot has no chain", async () => {
    const api = stubApi();
    mount(api, "/funding");
    expect(await screen.findByText(/Funding is off/)).toBeInTheDocument();
  });

  it("shows an account that could not be read with its reason", async () => {
    const api = funded({ accounts: [{ address: ADDRESS, error: "the chain did not answer", busy: false }] });
    mount(api, "/funding");
    expect(await screen.findByText("Not read: the chain did not answer")).toBeInTheDocument();
  });

  it("declares only after a class is chosen and its statement confirmed, for the selected accounts", async () => {
    const api = funded();
    api.declare.mockResolvedValue({ status: "ok", value: readJobs({ jobs: [jobBody(1), jobBody(2, { account: OTHER })] }) });
    const user = userEvent.setup();
    mount(api, "/funding");
    await select(user, ADDRESS);
    await select(user, OTHER);

    const button = screen.getByRole("button", { name: "Declare" });
    expect(button).toBeDisabled();
    await user.click(screen.getByRole("radio", { name: new RegExp(ELIGIBLE) }));
    expect(button).toBeDisabled();
    await user.click(screen.getByRole("checkbox", { name: /I have read the statement and confirm it for 2 selected accounts/ }));
    await user.click(button);

    expect(api.declare).toHaveBeenCalledExactlyOnceWith([ADDRESS, OTHER], "eligible_client", ELIGIBLE);
    expect(await screen.findByText("Started 2 jobs; their states are listed below.")).toBeInTheDocument();
  });

  it("asks for the confirmation again when another class is chosen", async () => {
    const user = userEvent.setup();
    mount(funded(), "/funding");
    await select(user, ADDRESS);
    await user.click(await screen.findByRole("radio", { name: new RegExp(PROFESSIONAL) }));
    await user.click(screen.getByRole("checkbox", { name: /I have read the statement/ }));
    await user.click(screen.getByRole("radio", { name: new RegExp(ELIGIBLE) }));
    expect(screen.getByRole("checkbox", { name: /I have read the statement/ })).not.toBeChecked();
    expect(screen.getByRole("button", { name: "Declare" })).toBeDisabled();
  });

  it("deposits an amount from the chosen wallet, offering the funding wallet only when there is one", async () => {
    const api = funded({ funding_key: false });
    const user = userEvent.setup();
    mount(api, "/funding");
    await select(user, ADDRESS);
    expect(await screen.findByRole("option", { name: "The account's own wallet" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "The funding wallet" })).not.toBeInTheDocument();
    await user.type(screen.getByLabelText("Amount of USDC for each account"), "250.5");
    await user.click(screen.getByRole("button", { name: "Deposit" }));
    expect(api.deposit).toHaveBeenCalledExactlyOnceWith([ADDRESS], "250.5", "account");
  });

  it("sends a funding-wallet deposit, a claim, and a withdrawal with and without an amount", async () => {
    const api = funded();
    const user = userEvent.setup();
    mount(api, "/funding");
    await select(user, OTHER);
    await user.type(await screen.findByLabelText("Amount of USDC for each account"), "5");
    await user.selectOptions(screen.getByLabelText("From"), "funding");
    await user.click(screen.getByRole("button", { name: "Deposit" }));
    expect(api.deposit).toHaveBeenCalledWith([OTHER], "5", "funding");

    await user.click(screen.getByRole("button", { name: "Claim" }));
    expect(api.claim).toHaveBeenCalledWith([OTHER]);

    await user.click(screen.getByRole("button", { name: "Withdraw" }));
    expect(api.withdraw).toHaveBeenLastCalledWith([OTHER], null);
    await user.type(screen.getByLabelText(/Amount of USDC from each account/), "100");
    await user.click(screen.getByRole("button", { name: "Withdraw" }));
    expect(api.withdraw).toHaveBeenLastCalledWith([OTHER], "100");
  });

  it("offers no action until an account is selected", async () => {
    const user = userEvent.setup();
    mount(funded(), "/funding");
    await screen.findByRole("region", { name: "Funding of each account" });
    for (const name of ["Declare", "Deposit", "Claim", "Withdraw"]) {
      expect(screen.getByRole("button", { name })).toBeDisabled();
    }
    await user.click(screen.getByRole("checkbox", { name: "Select every account" }));
    expect(screen.getByRole("button", { name: "Claim" })).toBeEnabled();
    await user.click(screen.getByRole("checkbox", { name: "Select every account" }));
    expect(screen.getByRole("button", { name: "Claim" })).toBeDisabled();
  });

  it("shows the bot's refusal under the form that caused it", async () => {
    const api = funded();
    api.claim.mockResolvedValue({ status: "refused", message: "0x1111 has an action that has not ended", code: "busy", retryAfterSeconds: null });
    const user = userEvent.setup();
    mount(api, "/funding");
    await select(user, ADDRESS);
    await user.click(screen.getByRole("button", { name: "Claim" }));
    expect(await screen.findByText("Failed: 0x1111 has an action that has not ended")).toBeInTheDocument();
  });

  it("reads again by itself until every job has ended, and shows each job's state and transactions", async () => {
    const api = stubApi();
    api.funding
      .mockResolvedValueOnce({ status: "ok", value: readFunding(fundingBody({ jobs: [jobBody(1, { action: "deposit", state: "running" })] })) })
      .mockResolvedValue({
        status: "ok",
        value: readFunding(fundingBody({ jobs: [jobBody(1, { action: "deposit", state: "landed", detail: "100 USDC credited", txs: ["0xfeed"] })] })),
      });
    mount(api, "/funding");

    const jobs = await screen.findByRole("region", { name: "Funding actions" });
    expect(within(jobs).getByText("Running")).toBeInTheDocument();
    await waitFor(() => { expect(within(jobs).getByText("Landed")).toBeInTheDocument(); }, { timeout: 5000 });
    expect(within(jobs).getByText("100 USDC credited")).toBeInTheDocument();
    expect(within(jobs).getByText("0xfeed")).toBeInTheDocument();
  });

  it("ends the session when the bot answers 401", async () => {
    const api = stubApi();
    api.funding.mockResolvedValue({ status: "unauthenticated" });
    mount(api, "/funding");
    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeInTheDocument();
  });

  it("links to the funding page from the overview", async () => {
    const user = userEvent.setup();
    mount(funded(), "/");
    await user.click(await screen.findByRole("link", { name: "Funding" }));
    expect(await screen.findByRole("heading", { level: 1, name: "Funding" })).toBeInTheDocument();
  });
});
