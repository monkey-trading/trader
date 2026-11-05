/** Tests of the page frame, login, and navigation between the pages. */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { figuresBody, overviewBody, totalsBody, viewBody } from "@/api/fixtures";
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

  it("shows an account without figures as unavailable with the reason, and says the totals cover fewer", async () => {
    const overview = overviewBody([viewBody({ figures: null, error: "the node did not answer" })], totalsBody({ accounts: 2, reporting: 1 }));
    mount(stubApi(), "/", overview);

    const table = await screen.findByRole("table", { name: "Accounts" });
    expect(within(table).getByText("Read failed: the node did not answer")).toBeInTheDocument();
    expect(within(table).getAllByRole("row")[1]?.textContent).toContain("—");
    expect(screen.getByText("Totals cover 1 of 2 accounts: the rest have no figures yet.")).toBeInTheDocument();
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
    expect(screen.getByRole("img", { name: /^Equity now 10,100.50 USDC/ })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "SMA crossover on BTC-USD" })).toBeInTheDocument();
    expect(screen.getByText("MONKEY claimable now").nextSibling?.textContent).toBe("5.00");
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
    mount(stubApi(), `/accounts/0x${"AB".repeat(20)}`, overviewBody([viewBody({ address: `0x${"ab".repeat(20)}` })]));

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
});
