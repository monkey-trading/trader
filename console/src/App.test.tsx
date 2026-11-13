/** Tests of the page frame, login, and navigation between the pages. */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { failedBody, figuresBody, overviewBody, totalsBody, viewBody } from "@/api/fixtures";
import { readAccount } from "@/api/parsers";
import { ADDRESS, mount, stubApi } from "@/test-harness";

describe("App", () => {
  it("frames the page with a skip link to the main region and one h1", async () => {
    mount(stubApi());

    expect(screen.getByRole("link", { name: "Skip to content" })).toHaveAttribute("href", "#main");
    expect(screen.getByRole("main")).toHaveAttribute("tabindex", "-1");
    expect(await screen.findByRole("heading", { level: 1, name: "Accounts" })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });

  it("asks for the password when there is no session and shows a refusal at the field", async () => {
    const api = stubApi();
    api.session.mockResolvedValue({ status: "unauthenticated" });
    api.login.mockResolvedValueOnce({ status: "unauthenticated" });
    const user = userEvent.setup();
    mount(api);

    await user.type(await screen.findByLabelText("Password"), "wrong{Enter}");

    expect(api.login).toHaveBeenCalledExactlyOnceWith("wrong");
    expect(await screen.findByText("The password was not accepted.")).toBeInTheDocument();
    expect(screen.getByLabelText("Password")).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("heading", { level: 1, name: "Sign in" })).not.toHaveFocus();

    await user.clear(screen.getByLabelText("Password"));
    await user.type(screen.getByLabelText("Password"), "right{Enter}");
    expect(await screen.findByRole("heading", { level: 1, name: "Accounts" })).toBeInTheDocument();
  });

  it("says the password is wrong when the bot's code says so", async () => {
    const api = stubApi();
    api.session.mockResolvedValue({ status: "unauthenticated" });
    api.login.mockResolvedValueOnce({ status: "unauthenticated", code: "wrong_password" });
    const user = userEvent.setup();
    mount(api);

    await user.type(await screen.findByLabelText("Password"), "wrong{Enter}");

    expect(await screen.findByText("Wrong password. Try again.")).toBeInTheDocument();
  });

  it("says how long to wait when the login is rate limited", async () => {
    const api = stubApi();
    api.session.mockResolvedValue({ status: "unauthenticated" });
    api.login.mockResolvedValue({ status: "refused", message: "too many attempts; try again later", retryAfterSeconds: 30 });
    const user = userEvent.setup();
    mount(api);

    await user.type(await screen.findByLabelText("Password"), "x{Enter}");

    expect(await screen.findByText("too many attempts; try again later Try again in 30 seconds.")).toBeInTheDocument();
  });

  it("shows every account's figures exactly as served, with the bot's totals", async () => {
    mount(stubApi());

    const table = await screen.findByRole("table", { name: "Accounts" });
    const [, row, total] = within(table).getAllByRole("row");
    expect(within(row ?? table).getAllByRole("cell").map((cell) => cell.textContent)).toEqual([
      "running", "10,100.50", "-3.25", "+100.50", "2,021.00%", "1", "12.50", "Not read yet",
    ]);
    expect(within(total ?? table).getAllByRole("cell").map((cell) => cell.textContent)).toEqual([
      "", "10,100.50", "-3.25", "+100.50", "2,021.00%", "1", "12.50", "Not read yet",
    ]);
  });

  it("shows an account without figures as unavailable with the reason, and how many accounts report", async () => {
    const overview = overviewBody([viewBody({ figures: null, error: "the node did not answer" })], totalsBody({ accounts: 2, reporting: 1 }));
    mount(stubApi(overview));

    const table = await screen.findByRole("table", { name: "Accounts" });
    expect(within(table).getByText("Read failed: the node did not answer")).toBeInTheDocument();
    expect(within(table).getAllByRole("row")[1]?.textContent).toContain("—");
    expect(screen.getByText("1 of 2 accounts reporting.")).toBeInTheDocument();
  });

  it("lists an account whose key would not unlock by its label, with the reason and no link", async () => {
    const overview = overviewBody([viewBody(), failedBody()], totalsBody({ accounts: 2, reporting: 1 }));
    mount(stubApi(overview));

    const table = await screen.findByRole("table", { name: "Accounts" });
    const rows = within(table).getAllByRole("row");
    expect(within(rows[2] ?? table).getByText("keys/locked.json")).toBeInTheDocument();
    expect(within(rows[2] ?? table).queryByRole("link")).not.toBeInTheDocument();
    expect(within(rows[2] ?? table).getByText("halted (the key would not unlock)")).toBeInTheDocument();
    expect(within(rows[2] ?? table).getByText("the keystore password is wrong")).toBeInTheDocument();
    expect(within(rows[1] ?? table).getByRole("link", { name: "0x1111…1111" })).toBeInTheDocument();
    expect(screen.getByText("1 of 2 accounts reporting.")).toBeInTheDocument();
  });

  it("lists two accounts that failed to start apart, by label, and an account that failed with an address links to its page", async () => {
    const overview = overviewBody(
      [failedBody({ label: "a.json" }), failedBody({ label: "b.json" }), viewBody({ state: "halted", halt: "no balance", code: "no_balance", figures: null, strategies: [] })],
      totalsBody({ accounts: 3, reporting: 0 }),
    );
    mount(stubApi(overview));

    const table = await screen.findByRole("table", { name: "Accounts" });
    expect(screen.getByText("a.json")).toBeInTheDocument();
    expect(screen.getByText("b.json")).toBeInTheDocument();
    expect(within(table).getAllByRole("link")).toHaveLength(1);
    expect(screen.getByText("0 of 3 accounts reporting.")).toBeInTheDocument();
  });

  it("shows an account the bot halted, with the reason, on the overview and on its page", async () => {
    const halted = viewBody({ state: "halted", halt: "an answer was signed by another key than the node's signer" });
    const user = userEvent.setup();
    mount(stubApi(overviewBody([halted])));

    expect(await screen.findByText("halted (an answer was signed by another key than the node's signer)")).toBeInTheDocument();
    await user.click(screen.getByRole("link", { name: "0x1111…1111" }));
    expect(await screen.findByText(/The account stopped on its own/)).toBeInTheDocument();
    expect(screen.getByText(/halted \(an answer was signed by another key/)).toBeInTheDocument();
  });

  it("shows the daily loss stop on the overview and the account page, and keeps the account's controls", async () => {
    const stopped = viewBody({ day_stop: { until_ms: Date.UTC(2026, 10, 6), loss: "100.5" } });
    const user = userEvent.setup();
    mount(stubApi(overviewBody([stopped])));

    const note = /^Stopped for the day: daily loss 100\.50 USD, trading resumes at .* local time \(UTC[+-]\d\d:\d\d\), 2026-11-06 00:00 UTC\.$/;
    expect(await screen.findByText(note)).toBeInTheDocument();
    await user.click(screen.getByRole("link", { name: "0x1111…1111" }));

    expect(await screen.findByText(note)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Stop account" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Stop strategy" })).toBeEnabled();
  });

  it("shows no stop note for an account without one", async () => {
    mount(stubApi());
    await screen.findByRole("table", { name: "Accounts" });
    expect(screen.queryByText(/Stopped for the day/)).not.toBeInTheDocument();
  });

  it("opens an account page from the overview and shows its tables, strategies and fills", async () => {
    const user = userEvent.setup();
    mount(stubApi());

    await user.click(await screen.findByRole("link", { name: "0x1111…1111" }));

    expect(window.location.pathname).toBe(`/accounts/${ADDRESS}`);
    expect(await screen.findByRole("heading", { level: 1, name: "0x1111…1111" })).toHaveFocus();
    expect(screen.getByRole("table", { name: "Open positions" })).toBeInTheDocument();
    expect(screen.getByRole("table", { name: "Resting orders" })).toBeInTheDocument();
    expect(await screen.findByRole("table", { name: "Recent fills" })).toBeInTheDocument();
    expect(screen.getByRole("img", { name: /^Equity now 10,100.50 USDC; lowest 10,000.00, highest 10,100.50 over 3 readings/ })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "SMA crossover on BTC-USD" })).toBeInTheDocument();
    expect(screen.getByText("MONKEY entitled to date").nextSibling?.textContent).toBe("5.00");
  });

  it("shows a position's mark, unrealised profit and close-out price", async () => {
    mount(stubApi(), `/accounts/${ADDRESS}`);

    const table = await screen.findByRole("table", { name: "Open positions" });
    expect(within(table).getAllByRole("row")[1]?.textContent).toBe("BTC-USDLong0.160,00061,000+100.0048,000");
  });

  it("says the bot has no such account once the snapshot has arrived", async () => {
    mount(stubApi(), `/accounts/0x${"22".repeat(20)}`);

    expect(await screen.findByText("The bot has no such account.")).toBeInTheDocument();
  });

  it("shows the figures of an account whose address the route spells in another case", async () => {
    mount(stubApi(overviewBody([viewBody({ address: `0x${"ab".repeat(20)}` })])), `/accounts/0x${"AB".repeat(20)}`);

    expect(await screen.findByRole("img", { name: /^Equity now/ })).toBeInTheDocument();
  });

  it("returns to the login page when the session check on a lost feed says 401", async () => {
    const api = stubApi();
    const { feeds } = mount(api);
    await screen.findByRole("heading", { name: "Accounts" });
    api.session.mockResolvedValue({ status: "unauthenticated" });

    feeds[0]?.onClose();

    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeInTheDocument();
  });

  it("says the feed is down and that the figures may be old, and never claims live", async () => {
    const { feeds } = mount(stubApi());
    await screen.findByRole("heading", { name: "Accounts" });
    expect(await screen.findByText("Live")).toBeInTheDocument();

    feeds[0]?.onClose();

    expect(await screen.findByText(/Disconnected\. Reconnecting/)).toBeInTheDocument();
    expect(screen.queryByText("Live")).not.toBeInTheDocument();
  });

  it("updates an account's figures from the feed with no refresh", async () => {
    const { feeds } = mount(stubApi());
    await screen.findByRole("table", { name: "Accounts" });

    feeds[0]?.onMessage(JSON.stringify({ type: "account", account: viewBody({ figures: figuresBody({ equity: "10200" }) }) }));

    await waitFor(() => { expect(screen.getAllByText("10,200.00").length).toBeGreaterThan(0); });
    expect(screen.queryByRole("button", { name: /refresh|live/i })).not.toBeInTheDocument();
  });

  it("signs out through the bot and shows the login page", async () => {
    const api = stubApi();
    const user = userEvent.setup();
    mount(api);

    await user.click(await screen.findByRole("button", { name: "Sign out" }));

    await waitFor(() => { expect(api.logout).toHaveBeenCalledTimes(1); });
    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeInTheDocument();
  });

  it("says the bot has not read an account yet, instead of showing empty positions and orders", async () => {
    const api = stubApi();
    api.account.mockResolvedValue({ status: "ok", value: readAccount(viewBody({ figures: null })) });
    mount(api, `/accounts/${ADDRESS}`);

    expect(await screen.findAllByText("Waiting for the bot to read this account.")).toHaveLength(2);
    expect(screen.queryByText("No open positions.")).not.toBeInTheDocument();
  });

  it("stays on the page and says so at the button when the bot did not end the session", async () => {
    const api = stubApi();
    api.logout.mockResolvedValue({ status: "unavailable", message: "The bot did not answer." });
    const user = userEvent.setup();
    mount(api);

    await user.click(await screen.findByRole("button", { name: "Sign out" }));

    expect(await screen.findByText("Failed: The bot did not end the session. The bot did not answer.")).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "Accounts" })).toBeInTheDocument();
  });
});
