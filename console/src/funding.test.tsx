/** Tests of the funding page: what it shows, and what each action sends for the accounts selected. */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { ADDRESS } from "@/api/fixtures";
import { ELIGIBLE, fundingBody, jobBody, OTHER, PROFESSIONAL } from "@/api/funding-fixtures";
import { readFunding, readJobs } from "@/api/funding-parsers";
import { BUSY_MS } from "@/components/use-funding";
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
    const api = funded({ accounts: [{ address: ADDRESS, error: "the chain did not answer", reading: false, busy: false }] });
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
    const claim = screen.getByRole("group", { name: "Claim MONKEY" });
    expect(await within(claim).findByText("Failed: 0x1111 has an action that has not ended")).toBeInTheDocument();
    expect(within(screen.getByRole("group", { name: "Withdraw" })).queryByText(/Failed/)).not.toBeInTheDocument();
  });

  it("reads again by itself until every job has ended, and shows each job's state and transactions", async () => {
    const api = stubApi();
    api.funding
      .mockResolvedValueOnce({ status: "ok", value: readFunding(fundingBody({ jobs: [jobBody(1, { action: "deposit", state: "running" })] })) })
      .mockResolvedValue({
        status: "ok",
        value: readFunding(fundingBody({ jobs: [jobBody(1, { action: "deposit", state: "landed", detail: "100 USDC credited", txs: [{ hash: "0xfeed", what: "depositing 100 USDC", state: "mined" }] })] })),
      });
    mount(api, "/funding");

    const jobs = await screen.findByRole("region", { name: "Funding actions" });
    expect(within(jobs).getByText("Running")).toBeInTheDocument();
    await waitFor(() => { expect(within(jobs).getByText("Landed")).toBeInTheDocument(); }, { timeout: 5000 });
    expect(within(jobs).getByText("100 USDC credited")).toBeInTheDocument();
    expect(within(jobs).getByText("0xfeed")).toBeInTheDocument();
    expect(within(jobs).getByText(/depositing 100 USDC: mined/)).toBeInTheDocument();
  });

  it("ends the session when the bot answers 401", async () => {
    const api = stubApi();
    api.funding.mockResolvedValue({ status: "unauthenticated" });
    mount(api, "/funding");
    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeInTheDocument();
  });

  it("asks for the confirmation again when the selection changes, and after a request", async () => {
    const api = funded();
    api.declare.mockResolvedValue({ status: "ok", value: [] });
    const user = userEvent.setup();
    mount(api, "/funding");
    await select(user, ADDRESS);
    await user.click(await screen.findByRole("radio", { name: new RegExp(PROFESSIONAL) }));
    const confirm = () => screen.getByRole("checkbox", { name: /I have read the statement/ });
    await user.click(confirm());
    expect(screen.getByRole("button", { name: "Declare" })).toBeEnabled();

    await select(user, OTHER);
    expect(confirm()).not.toBeChecked();
    expect(screen.getByRole("button", { name: "Declare" })).toBeDisabled();

    await user.click(confirm());
    await user.click(screen.getByRole("button", { name: "Declare" }));
    expect(api.declare).toHaveBeenCalledExactlyOnceWith([ADDRESS, OTHER], "professional", PROFESSIONAL);
    await waitFor(() => { expect(confirm()).not.toBeChecked(); });
    expect(screen.getByRole("button", { name: "Declare" })).toBeDisabled();
  });

  it("shows an account the bot has not read yet as being read", async () => {
    const api = funded({ accounts: [{ address: ADDRESS, error: null, reading: true, busy: false }] });
    mount(api, "/funding");
    expect(await screen.findByText("Reading…")).toBeInTheDocument();
  });

  it("announces a job's end once it ends", async () => {
    const api = stubApi();
    api.funding
      .mockResolvedValueOnce({ status: "ok", value: readFunding(fundingBody({ jobs: [jobBody(1, { action: "claim", state: "running" })] })) })
      .mockResolvedValue({
        status: "ok",
        value: readFunding(fundingBody({ jobs: [jobBody(1, { action: "claim", state: "failed", detail: "the chain did not answer" })] })),
      });
    mount(api, "/funding");
    await screen.findByRole("region", { name: "Funding actions" });
    const said = await screen.findByText(/claim for 0x1111…1111 failed: the chain did not answer/, {}, { timeout: 5000 });
    expect(said).toHaveAttribute("aria-live", "polite");
  });

  it("reads nothing while the page is hidden, and reads again once it is shown", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    let hidden = true;
    Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
    try {
      // A job that has not ended would have the page read every BUSY_MS while shown.
      const api = funded({ jobs: [jobBody(1, { state: "running" })] });
      mount(api, "/funding");
      await vi.advanceTimersByTimeAsync(0);
      const first = api.funding.mock.calls.length;
      expect(first).toBeGreaterThan(0);
      await vi.advanceTimersByTimeAsync(20 * BUSY_MS);
      expect(api.funding).toHaveBeenCalledTimes(first);

      hidden = false;
      document.dispatchEvent(new Event("visibilitychange"));
      await vi.advanceTimersByTimeAsync(0);
      expect(api.funding).toHaveBeenCalledTimes(first + 1);
    } finally {
      Reflect.deleteProperty(document, "hidden");
      vi.useRealTimers();
    }
  });

  it("links to the funding page from the overview", async () => {
    const user = userEvent.setup();
    mount(funded(), "/");
    await user.click(await screen.findByRole("link", { name: "Funding" }));
    expect(await screen.findByRole("heading", { level: 1, name: "Funding" })).toBeInTheDocument();
  });
});
